//! Mainnet account dumping: the store, the RPC shape, and the resolution the
//! [`Dump`](crate::fixture::Dump) fixture drives.
//!
//! # What a dump is
//!
//! A dump copies real accounts (or a real program) from a live cluster into a
//! test world so a fixture can exercise on-chain state the harness would
//! otherwise have to hand-build. The copied bytes are cached in a committed
//! `.parallax/` store next to the consuming project so that, once warm, a test
//! is fully offline and deterministic.
//!
//! # The core / shell boundary (why two-phase)
//!
//! Every dump *semantic* lives here in the core: the store format, the coherence
//! rules, the JSON-RPC request/response shape, and account/program installation.
//! The one thing that is inherently frontend-specific is the network transport —
//! and the FFI consumer (the TypeScript shell) must perform network I/O with JS
//! `fetch`, which is asynchronous, while the FFI boundary is synchronous.
//!
//! So the core exposes resolution as two synchronous steps:
//!
//! 1. [`Test::dump_plan`] reads the store, installs every cache **hit**, and —
//!    when there are **misses** — returns the exact JSON-RPC request body to
//!    POST. It performs no I/O of its own.
//! 2. The frontend transports that body however it likes and hands the response
//!    back to [`Test::dump_commit`], which parses it, writes the store, and
//!    installs the fetched accounts.
//!
//! A native Rust test never sees the seam: [`Test::add`](crate::Test::add) of a
//! `Dump` runs both steps back to back, using the built-in [`UreqTransport`]
//! (the `native-rpc` feature) between them. The FFI cdylib is built without that
//! feature, so it carries no TLS stack and opens no socket; the TypeScript shell
//! supplies `fetch` between `parallax_dump_plan` and `parallax_dump_commit`.
//! Either way the store format and every coherence rule are single-sourced here.

use {
    crate::{world::Test, Account, Pubkey},
    base64::{engine::general_purpose::STANDARD, Engine as _},
    serde_json::{json, Value},
    solana_sdk_ids::bpf_loader_upgradeable,
    std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        str::FromStr,
    },
};

/// Default endpoint used when a world sets no [`rpc`](crate::TestBuilder::rpc):
/// the public mainnet-beta RPC. This is a code-only default — there is
/// deliberately no environment-variable override.
pub(crate) const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";

/// Store layout version, written into every store file for forward evolution.
const STORE_VERSION: u64 = 1;

/// Directory (next to the project manifest) that holds the committed store.
const STORE_DIR: &str = ".parallax";

/// The single store file inside [`STORE_DIR`].
const STORE_FILE: &str = "accounts.json";

/// Mixed-slot coherence threshold: one mainnet epoch (432,000 slots, ~2–3
/// days). Entries whose observed slots span more than this are unlikely to be a
/// coherent snapshot, so combining them warns once and points at
/// [`Dump::refresh_all`](crate::fixture::Dump::refresh_all).
const EPOCH_SLOTS: u64 = 432_000;

/// Loader-v3 `ProgramData` metadata header length preceding the ELF: a 4-byte
/// enum tag, an 8-byte slot, and a 33-byte `Option<Pubkey>` upgrade authority.
/// The runtime always reserves the full 45 bytes, so the ELF starts at offset
/// 45 regardless of whether an upgrade authority is present.
const PROGRAMDATA_HEADER_LEN: usize = 45;

/// Mainnet-beta genesis creation time (2020-03-16 14:29:00 UTC), the anchor for
/// [`sync_clock`](crate::fixture::Dump::sync_clock)'s slot-derived timestamp.
const MAINNET_GENESIS_UNIX: i64 = 1_584_368_940;

/// Approximate mainnet slot time, used to derive a wall-clock from a slot.
const MS_PER_SLOT: i64 = 400;

// Role codes on the FFI boundary; `Role` itself stays internal.
pub(crate) const ROLE_ACCOUNT: u8 = 0;
pub(crate) const ROLE_PROGRAM: u8 = 1;
pub(crate) const ROLE_PROGRAMDATA: u8 = 2;

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// The network transport a dump uses to fetch account bytes on a store miss.
///
/// The core builds the JSON-RPC request and parses the response; a transport
/// only moves bytes. This lets tests inject a recorded transport and lets the
/// FFI omit network I/O entirely.
pub(crate) trait DumpTransport: Send {
    /// POST `request_body` to `url` and return the raw response body.
    fn fetch(&self, url: &str, request_body: &[u8]) -> Result<Vec<u8>, String>;
}

/// The default transport in a build without `native-rpc` (notably the FFI
/// cdylib): it never runs, because that path resolves misses through the
/// two-phase plan/commit wire, and errors clearly if somehow invoked.
#[cfg(not(feature = "native-rpc"))]
pub(crate) struct DisabledTransport;

#[cfg(not(feature = "native-rpc"))]
impl DumpTransport for DisabledTransport {
    fn fetch(&self, _url: &str, _request_body: &[u8]) -> Result<Vec<u8>, String> {
        Err(
            "this build has no RPC transport; native Rust builds enable the \
             `native-rpc` feature, and the TypeScript harness supplies its own \
             `fetch` transport"
                .into(),
        )
    }
}

/// A minimal blocking HTTPS JSON-RPC transport backed by `ureq` + rustls.
#[cfg(feature = "native-rpc")]
pub(crate) struct UreqTransport;

#[cfg(feature = "native-rpc")]
impl DumpTransport for UreqTransport {
    fn fetch(&self, url: &str, request_body: &[u8]) -> Result<Vec<u8>, String> {
        use std::io::Read as _;
        let response = ureq::post(url)
            .set("content-type", "application/json")
            .send_bytes(request_body)
            .map_err(|error| format!("RPC request to {url} failed: {error}"))?;
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|error| format!("reading RPC response from {url} failed: {error}"))?;
        Ok(body)
    }
}

/// The world's default transport for the active build.
pub(crate) fn default_transport() -> Box<dyn DumpTransport> {
    #[cfg(feature = "native-rpc")]
    {
        Box::new(UreqTransport)
    }
    #[cfg(not(feature = "native-rpc"))]
    {
        Box::new(DisabledTransport)
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// How an entry is reinstalled into a world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// A plain account, installed with `set_account`.
    Account,
    /// A program's executable account; installed by loading its ELF.
    Program,
    /// A loader-v3 `ProgramData` account; carried for its program's ELF.
    ProgramData,
}

impl Role {
    fn from_code(code: u8) -> Result<Self, String> {
        match code {
            ROLE_ACCOUNT => Ok(Self::Account),
            ROLE_PROGRAM => Ok(Self::Program),
            ROLE_PROGRAMDATA => Ok(Self::ProgramData),
            other => Err(format!("dump: unknown role code {other}")),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Account => ROLE_ACCOUNT,
            Self::Program => ROLE_PROGRAM,
            Self::ProgramData => ROLE_PROGRAMDATA,
        }
    }

    fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Account => None,
            Self::Program => Some("program"),
            Self::ProgramData => Some("programdata"),
        }
    }
}

/// One stored account: its observed slot plus the raw account fields.
#[derive(Clone)]
pub(crate) struct StoredAccount {
    slot: u64,
    lamports: u64,
    owner: Pubkey,
    executable: bool,
    data: Vec<u8>,
    role: Role,
}

/// The committed `.parallax/accounts.json` store, keyed by base58 address.
///
/// A single sorted-key JSON file: compact (base64 data), diff-friendly (stable
/// key order, one block per address), and language-neutral. Only the core reads
/// or writes it, so the format has exactly one implementation.
pub(crate) struct DumpStore {
    path: PathBuf,
    accounts: BTreeMap<String, StoredAccount>,
}

impl DumpStore {
    fn path_for(project_dir: &Path) -> PathBuf {
        project_dir.join(STORE_DIR).join(STORE_FILE)
    }

    pub(crate) fn load(project_dir: &Path) -> Result<Self, String> {
        let path = Self::path_for(project_dir);
        let accounts = if path.exists() {
            let text = fs::read_to_string(&path).map_err(|error| {
                format!("dump: could not read store {}: {error}", path.display())
            })?;
            let value: Value = serde_json::from_str(&text).map_err(|error| {
                format!("dump: store {} is not valid JSON: {error}", path.display())
            })?;
            parse_store(&value)?
        } else {
            BTreeMap::new()
        };
        Ok(Self { path, accounts })
    }

    fn get(&self, address: &Pubkey) -> Option<&StoredAccount> {
        self.accounts.get(&address.to_string())
    }

    fn put(&mut self, address: Pubkey, entry: StoredAccount) {
        self.accounts.insert(address.to_string(), entry);
    }

    /// Every stored entry as an install target, preserving each entry's role.
    fn targets(&self) -> Result<Vec<(Pubkey, Role)>, String> {
        self.accounts
            .iter()
            .map(|(address, entry)| {
                Pubkey::from_str(address)
                    .map(|address| (address, entry.role))
                    .map_err(|error| {
                        format!("dump: store has an invalid address {address}: {error}")
                    })
            })
            .collect()
    }

    fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&serialize_store(&self.accounts))
            .map_err(|error| format!("dump: could not serialize store: {error}"))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("dump: could not create {}: {error}", parent.display()))?;
        }
        fs::write(&self.path, format!("{text}\n")).map_err(|error| {
            format!(
                "dump: could not write store {}: {error}",
                self.path.display()
            )
        })
    }
}

fn parse_store(value: &Value) -> Result<BTreeMap<String, StoredAccount>, String> {
    let accounts = value
        .get("accounts")
        .and_then(Value::as_object)
        .ok_or("dump: store is missing an `accounts` object")?;
    let mut out = BTreeMap::new();
    for (address, item) in accounts {
        let slot = field_u64(item, "slot")?;
        let lamports = field_u64(item, "lamports")?;
        let owner = field_pubkey(item, "owner")?;
        let executable = item
            .get("executable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = STANDARD
            .decode(field_str(item, "data")?)
            .map_err(|error| format!("dump: store entry {address} has invalid base64: {error}"))?;
        let role = match item.get("role").and_then(Value::as_str) {
            None => Role::Account,
            Some("program") => Role::Program,
            Some("programdata") => Role::ProgramData,
            Some(other) => {
                return Err(format!(
                    "dump: store entry {address} has unknown role {other}"
                ))
            }
        };
        out.insert(
            address.clone(),
            StoredAccount {
                slot,
                lamports,
                owner,
                executable,
                data,
                role,
            },
        );
    }
    Ok(out)
}

fn serialize_store(accounts: &BTreeMap<String, StoredAccount>) -> Value {
    let mut map = serde_json::Map::new();
    for (address, entry) in accounts {
        let mut object = serde_json::Map::new();
        object.insert("slot".into(), json!(entry.slot));
        object.insert("lamports".into(), json!(entry.lamports));
        object.insert("owner".into(), json!(entry.owner.to_string()));
        object.insert("executable".into(), json!(entry.executable));
        object.insert("data".into(), json!(STANDARD.encode(&entry.data)));
        if let Some(role) = entry.role.as_str() {
            object.insert("role".into(), json!(role));
        }
        map.insert(address.clone(), Value::Object(object));
    }
    json!({ "version": STORE_VERSION, "accounts": Value::Object(map) })
}

// ---------------------------------------------------------------------------
// JSON-RPC shape (getMultipleAccounts, base64, one observed slot)
// ---------------------------------------------------------------------------

/// One account fetched from the cluster (no slot; the slot is per-batch).
struct FetchedAccount {
    lamports: u64,
    owner: Pubkey,
    executable: bool,
    data: Vec<u8>,
}

/// Build a single batched `getMultipleAccounts` request (base64 encoding) for
/// every miss address at once — the whole array observed at one slot.
pub(crate) fn build_request(addresses: &[Pubkey]) -> Vec<u8> {
    let encoded: Vec<String> = addresses.iter().map(Pubkey::to_string).collect();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [encoded, { "encoding": "base64" }],
    });
    serde_json::to_vec(&body).expect("serializing a JSON-RPC request never fails")
}

/// Parse a `getMultipleAccounts` response into the observed slot and one entry
/// per requested address (`None` where the account does not exist on chain).
fn parse_response(
    addresses: &[Pubkey],
    body: &[u8],
) -> Result<(u64, Vec<Option<FetchedAccount>>), String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("dump: RPC response was not JSON: {error}"))?;
    if let Some(error) = value.get("error") {
        return Err(format!("dump: RPC returned an error: {error}"));
    }
    let result = value
        .get("result")
        .ok_or("dump: RPC response is missing `result`")?;
    let slot = result
        .get("context")
        .and_then(|context| context.get("slot"))
        .and_then(Value::as_u64)
        .ok_or("dump: RPC response is missing the context slot")?;
    let values = result
        .get("value")
        .and_then(Value::as_array)
        .ok_or("dump: RPC response is missing the `value` array")?;
    if values.len() != addresses.len() {
        return Err(format!(
            "dump: RPC returned {} accounts for {} requested addresses",
            values.len(),
            addresses.len()
        ));
    }
    let mut out = Vec::with_capacity(values.len());
    for item in values {
        if item.is_null() {
            out.push(None);
            continue;
        }
        let lamports = field_u64(item, "lamports")?;
        let owner = field_pubkey(item, "owner")?;
        let executable = item
            .get("executable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = decode_rpc_data(item.get("data").ok_or("dump: account is missing `data`")?)?;
        out.push(Some(FetchedAccount {
            lamports,
            owner,
            executable,
            data,
        }));
    }
    Ok((slot, out))
}

/// Decode `getMultipleAccounts`' base64 data field (`["<base64>", "base64"]`,
/// or a bare string for tolerance).
fn decode_rpc_data(field: &Value) -> Result<Vec<u8>, String> {
    let encoded = match field {
        Value::Array(parts) => parts
            .first()
            .and_then(Value::as_str)
            .ok_or("dump: account data array is malformed")?,
        Value::String(text) => text.as_str(),
        _ => return Err("dump: account data is neither an array nor a string".into()),
    };
    STANDARD
        .decode(encoded)
        .map_err(|error| format!("dump: account data is not valid base64: {error}"))
}

fn field_u64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("dump: missing or non-integer `{key}`"))
}

fn field_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("dump: missing or non-string `{key}`"))
}

fn field_pubkey(value: &Value, key: &str) -> Result<Pubkey, String> {
    let text = field_str(value, key)?;
    Pubkey::from_str(text)
        .map_err(|error| format!("dump: `{key}` is not an address ({text}): {error}"))
}

/// Loader-v3 programdata address for `program_id`.
pub(crate) fn programdata_address(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program_id.as_ref()], &bpf_loader_upgradeable::ID).0
}

// ---------------------------------------------------------------------------
// Resolution (installed into the world)
// ---------------------------------------------------------------------------

/// Result of [`Test::dump_plan`]: the request body to POST (empty when nothing
/// needs fetching) and the misses it covers, each with its role code.
#[doc(hidden)]
pub struct DumpPlan {
    /// JSON-RPC request body to POST, or empty when there are no misses.
    pub request_body: Vec<u8>,
    /// Miss `(address, role_code)` pairs, in request order.
    pub misses: Vec<(Pubkey, u8)>,
}

impl Test {
    /// Phase one of a dump. Reads the store, installs every cache hit, and
    /// returns the request body for the misses (empty when the store already
    /// covers every target). When `refresh` is set, the store's own entries are
    /// the targets and all of them are treated as misses.
    ///
    /// Internal FFI support; not a stable API.
    #[doc(hidden)]
    pub fn dump_plan(
        &mut self,
        project_dir: &str,
        targets: &[(Pubkey, u8)],
        sync_clock: bool,
        refresh: bool,
    ) -> Result<DumpPlan, String> {
        let dir = Path::new(project_dir);
        let store = DumpStore::load(dir)?;
        let targets = if refresh {
            store.targets()?
        } else {
            // A `Program` target expands to include its loader-v3 programdata, so
            // the programdata derivation stays here in the core and no frontend
            // (native or the FFI shell) has to reproduce it.
            let mut expanded = Vec::new();
            for (address, code) in targets {
                let role = Role::from_code(*code)?;
                expanded.push((*address, role));
                if role == Role::Program {
                    expanded.push((programdata_address(address), Role::ProgramData));
                }
            }
            expanded
        };

        let mut hits = Vec::new();
        let mut misses = Vec::new();
        for (address, role) in targets {
            match (refresh, store.get(&address)) {
                (false, Some(entry)) => hits.push((address, entry.clone())),
                _ => misses.push((address, role)),
            }
        }

        self.install_entries(&hits)?;
        self.record_dumped(&hits);

        if misses.is_empty() {
            self.finish_dump(sync_clock, hits.iter().map(|(_, entry)| entry.slot).max());
            return Ok(DumpPlan {
                request_body: Vec::new(),
                misses: Vec::new(),
            });
        }

        let request_body = build_request(
            &misses
                .iter()
                .map(|(address, _)| *address)
                .collect::<Vec<_>>(),
        );
        Ok(DumpPlan {
            request_body,
            misses: misses
                .into_iter()
                .map(|(address, role)| (address, role.code()))
                .collect(),
        })
    }

    /// Phase two of a dump. Parses the RPC response for the `misses` returned by
    /// [`Self::dump_plan`], writes the store, installs the fetched accounts, and
    /// applies `sync_clock` from the observed slot.
    ///
    /// Internal FFI support; not a stable API.
    #[doc(hidden)]
    pub fn dump_commit(
        &mut self,
        project_dir: &str,
        misses: &[(Pubkey, u8)],
        response_body: &[u8],
        sync_clock: bool,
    ) -> Result<(), String> {
        let dir = Path::new(project_dir);
        let mut store = DumpStore::load(dir)?;
        let addresses: Vec<Pubkey> = misses.iter().map(|(address, _)| *address).collect();
        let (slot, values) = parse_response(&addresses, response_body)?;

        let mut installed = Vec::new();
        for ((address, code), value) in misses.iter().zip(values) {
            let role = Role::from_code(*code)?;
            match value {
                Some(fetched) => {
                    let entry = StoredAccount {
                        slot,
                        lamports: fetched.lamports,
                        owner: fetched.owner,
                        executable: fetched.executable,
                        data: fetched.data,
                        role,
                    };
                    store.put(*address, entry.clone());
                    installed.push((*address, entry));
                }
                None => eprintln!(
                    "parallax dump: account {address} does not exist on chain; skipped \
                     (dump only real addresses)"
                ),
            }
        }
        store.save()?;
        self.install_entries(&installed)?;
        self.record_dumped(&installed);
        eprintln!(
            "parallax: dumped {} account(s) @ slot {slot}",
            installed.len()
        );
        self.finish_dump(sync_clock, Some(slot));
        Ok(())
    }

    /// Install a set of resolved store entries into the world. Plain accounts
    /// are set directly; a program account is made executable by loading the ELF
    /// (from its paired loader-v3 programdata, or from the account itself for the
    /// older loaders); programdata entries are consumed by their program.
    fn install_entries(&mut self, entries: &[(Pubkey, StoredAccount)]) -> Result<(), String> {
        let by_address: BTreeMap<Pubkey, &StoredAccount> = entries
            .iter()
            .map(|(address, entry)| (*address, entry))
            .collect();
        for (address, entry) in entries {
            match entry.role {
                Role::Account => self.backend.set_account(Account {
                    address: *address,
                    lamports: entry.lamports,
                    data: entry.data.clone(),
                    owner: entry.owner,
                    executable: entry.executable,
                }),
                // The program's ELF is loaded through its `Program` entry.
                Role::ProgramData => {}
                Role::Program => {
                    let (elf, loader) = if entry.owner == bpf_loader_upgradeable::ID {
                        let programdata = programdata_address(address);
                        let source = by_address.get(&programdata).ok_or_else(|| {
                            format!(
                                "dump: program {address} is loader-v3 but its programdata \
                                 {programdata} was not dumped"
                            )
                        })?;
                        let elf = source.data.get(PROGRAMDATA_HEADER_LEN..).ok_or_else(|| {
                            format!("dump: programdata {programdata} is too small to hold an ELF")
                        })?;
                        (elf.to_vec(), bpf_loader_upgradeable::ID)
                    } else {
                        (entry.data.clone(), entry.owner)
                    };
                    self.backend
                        .load_program_with_loader(address, &elf, loader)?;
                }
            }
        }
        Ok(())
    }

    /// Record installed dump entries for coherence tracking and guided errors.
    fn record_dumped(&mut self, entries: &[(Pubkey, StoredAccount)]) {
        for (address, entry) in entries {
            self.dumped_addresses.push(*address);
            self.dumped_slots.push(entry.slot);
        }
    }

    /// Apply `sync_clock` (from the most recent touched slot) and emit the
    /// mixed-slot coherence warning at most once per world.
    fn finish_dump(&mut self, sync_clock: bool, slot: Option<u64>) {
        if sync_clock {
            if let Some(slot) = slot {
                let timestamp = MAINNET_GENESIS_UNIX + (slot as i64) * MS_PER_SLOT / 1000;
                self.backend.sync_clock(slot, timestamp);
            }
        }
        if self.dump_warned {
            return;
        }
        if let Some(warning) = coherence_warning(&self.dumped_slots, EPOCH_SLOTS) {
            self.dump_warned = true;
            eprintln!("{warning}");
        }
    }

    /// Whether this world has any dumped accounts (drives guided errors).
    pub(crate) fn has_dumps(&self) -> bool {
        !self.dumped_addresses.is_empty()
    }

    // --- Native (single-call) resolution -----------------------------------

    /// Native path for `Dump::accounts`: plan, fetch any misses through the
    /// built-in transport, commit. Panics with an actionable message on failure,
    /// matching the rest of fixture setup.
    pub(crate) fn dump_accounts_native(&mut self, addresses: &[Pubkey], sync_clock: bool) {
        let targets: Vec<(Pubkey, u8)> = addresses
            .iter()
            .map(|address| (*address, ROLE_ACCOUNT))
            .collect();
        self.run_dump_native(&targets, sync_clock, false);
    }

    /// Native path for `Dump::program`: dump the program account and its
    /// loader-v3 programdata coherently, then load the program. `dump_plan`
    /// expands the `Program` target to include the programdata.
    pub(crate) fn dump_program_native(&mut self, program_id: Pubkey, sync_clock: bool) {
        self.run_dump_native(&[(program_id, ROLE_PROGRAM)], sync_clock, false);
    }

    /// Native path for `Dump::refresh_all`: re-fetch every known entry in one
    /// coherent batch. Returns the refreshed addresses. `dump_plan` with
    /// `refresh` reads the store itself, so the targets passed here are ignored.
    pub(crate) fn refresh_all_native(&mut self) -> Vec<Pubkey> {
        self.run_dump_native(&[], false, true)
    }

    fn run_dump_native(
        &mut self,
        targets: &[(Pubkey, u8)],
        sync_clock: bool,
        refresh: bool,
    ) -> Vec<Pubkey> {
        let dir = self.project_dir_string();
        let plan = self
            .dump_plan(&dir, targets, sync_clock, refresh)
            .unwrap_or_else(|error| panic!("{error}"));
        // With `refresh`, the resolved set is exactly the misses (every stored
        // entry); otherwise it is the requested targets.
        let resolved: Vec<Pubkey> = if refresh {
            plan.misses.iter().map(|(address, _)| *address).collect()
        } else {
            targets.iter().map(|(address, _)| *address).collect()
        };
        if plan.misses.is_empty() {
            return resolved;
        }
        let url = self.rpc_url.clone();
        let response = self
            .transport
            .fetch(&url, &plan.request_body)
            .unwrap_or_else(|error| panic!("parallax dump: {error}"));
        self.dump_commit(&dir, &plan.misses, &response, sync_clock)
            .unwrap_or_else(|error| panic!("parallax dump: {error}"));
        resolved
    }

    /// Resolve the project directory whose `.parallax/` store this world uses:
    /// the builder-set directory (the `#[parallax_test]` macro passes
    /// `CARGO_MANIFEST_DIR`), then the `CARGO_MANIFEST_DIR` runtime variable,
    /// then the nearest ancestor of the working directory that has a
    /// `Cargo.toml`.
    fn project_dir_string(&self) -> String {
        if let Some(dir) = &self.project_dir {
            return dir.clone();
        }
        if let Some(dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
            return dir.to_string_lossy().into_owned();
        }
        if let Ok(cwd) = std::env::current_dir() {
            for ancestor in cwd.ancestors() {
                if ancestor.join("Cargo.toml").is_file() {
                    return ancestor.to_string_lossy().into_owned();
                }
            }
            return cwd.to_string_lossy().into_owned();
        }
        ".".into()
    }
}

/// The mixed-slot coherence warning for a set of observed slots, or `None` when
/// they fall within `threshold` of each other. Pure so it can be tested without
/// capturing stderr.
fn coherence_warning(slots: &[u64], threshold: u64) -> Option<String> {
    let min = slots.iter().min()?;
    let max = slots.iter().max()?;
    (max - min > threshold).then(|| {
        format!(
            "parallax dump: combining accounts across a {}-slot range ({min}..={max}, more than \
             one epoch) — the world may not be a coherent snapshot; call Dump::refresh_all() to \
             re-fetch every entry at one slot",
            max - min
        )
    })
}

/// The guided-error hint appended when a transaction fails on an account the
/// world never installed, in a world that has dumped accounts. Returns `None`
/// unless `has_dumps` and a named read-only account is genuinely absent.
pub(crate) fn missing_account_hint(missing: Option<Pubkey>) -> Option<String> {
    missing.map(|address| {
        format!(
            "missing account {address} — if it exists on mainnet, add it to \
             Dump::accounts([...])"
        )
    })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{fixture::Dump, AccountMeta, Instruction, Test},
        std::sync::Mutex,
    };

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "parallax-dump-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A `getMultipleAccounts` response for `accounts` at `slot`, in order.
    fn account_response(slot: u64, accounts: &[(Pubkey, u64, Vec<u8>, bool)]) -> Vec<u8> {
        let values: Vec<Value> = accounts
            .iter()
            .map(|(owner, lamports, data, executable)| {
                json!({
                    "lamports": lamports,
                    "owner": owner.to_string(),
                    "executable": executable,
                    "data": [STANDARD.encode(data), "base64"],
                    "rentEpoch": 0,
                })
            })
            .collect();
        serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "context": { "slot": slot }, "value": values },
        }))
        .unwrap()
    }

    fn world(dir: &Path, transport: Box<dyn DumpTransport>) -> Test {
        Test::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .project_dir(dir.to_string_lossy().into_owned())
            .transport(transport)
            .build()
            .unwrap()
    }

    /// A transport that replays queued responses (one per fetch, in order).
    struct RecordedTransport {
        responses: Mutex<Vec<Vec<u8>>>,
    }

    impl RecordedTransport {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }
    }

    impl DumpTransport for RecordedTransport {
        fn fetch(&self, _url: &str, _request_body: &[u8]) -> Result<Vec<u8>, String> {
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| "no recorded response".into())
        }
    }

    /// A transport that fails if the network is ever touched — proves warm runs
    /// stay offline.
    struct PanicTransport;

    impl DumpTransport for PanicTransport {
        fn fetch(&self, _url: &str, _request_body: &[u8]) -> Result<Vec<u8>, String> {
            panic!("a warm dump must not touch the network");
        }
    }

    #[test]
    fn build_request_is_one_batched_base64_call() {
        let body = build_request(&[
            Pubkey::new_from_array([1; 32]),
            Pubkey::new_from_array([2; 32]),
        ]);
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["method"], "getMultipleAccounts");
        assert_eq!(value["params"][0].as_array().unwrap().len(), 2);
        assert_eq!(value["params"][1]["encoding"], "base64");
    }

    #[test]
    fn parse_response_captures_slot_and_missing_accounts() {
        let owner = Pubkey::new_from_array([9; 32]).to_string();
        let data = STANDARD.encode([1, 2, 3]);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": { "slot": 314 },
                "value": [
                    { "lamports": 5, "owner": owner, "executable": false, "data": [data, "base64"] },
                    null,
                ],
            },
        });
        let addresses = [
            Pubkey::new_from_array([1; 32]),
            Pubkey::new_from_array([2; 32]),
        ];
        let (slot, values) =
            parse_response(&addresses, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(slot, 314);
        assert_eq!(values[0].as_ref().unwrap().data, vec![1, 2, 3]);
        assert_eq!(values[0].as_ref().unwrap().lamports, 5);
        assert!(values[1].is_none());
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!(
            "parallax-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let mut store = DumpStore::load(&dir).unwrap();
        let address = Pubkey::new_from_array([7; 32]);
        store.put(
            address,
            StoredAccount {
                slot: 42,
                lamports: 1000,
                owner: Pubkey::new_from_array([3; 32]),
                executable: false,
                data: vec![4, 5, 6],
                role: Role::Account,
            },
        );
        store.save().unwrap();

        let reloaded = DumpStore::load(&dir).unwrap();
        let entry = reloaded.get(&address).unwrap();
        assert_eq!(entry.slot, 42);
        assert_eq!(entry.data, vec![4, 5, 6]);
        assert_eq!(entry.role, Role::Account);
        // The file exists next to the (temp) project dir, under `.parallax/`.
        assert!(dir.join(STORE_DIR).join(STORE_FILE).is_file());
        fs::remove_dir_all(&dir).unwrap();
    }

    // A miss fetches once and writes the store; a second world reading the same
    // warm store installs the identical account without touching the network —
    // the determinism-on-warm-store property.
    #[test]
    fn warm_store_is_offline_and_deterministic() {
        let dir = temp_project("warm");
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([6; 32]);
        let data = vec![1, 2, 3, 4];
        let response = account_response(1_000, &[(owner, 777, data.clone(), false)]);

        {
            let mut test = world(&dir, Box::new(RecordedTransport::new(vec![response])));
            let [got] = test.add(Dump::accounts([address]));
            assert_eq!(got, address);
            let account = test.account(address).unwrap();
            assert_eq!(account.owner, owner);
            assert_eq!(account.lamports, 777);
            assert_eq!(account.data, data);
        }
        {
            // The transport panics if the warm run touches the network at all.
            let mut test = world(&dir, Box::new(PanicTransport));
            let [got] = test.add(Dump::accounts([address]));
            assert_eq!(got, address);
            assert_eq!(test.account(address).unwrap().data, data);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_clock_adopts_the_dumped_slot() {
        let dir = temp_project("clock");
        let address = Pubkey::new_from_array([5; 32]);
        let response = account_response(
            2_000,
            &[(Pubkey::new_from_array([6; 32]), 1, vec![], false)],
        );
        let mut test = world(&dir, Box::new(RecordedTransport::new(vec![response])));

        test.add(Dump::accounts([address]).sync_clock());
        let (slot, timestamp) = test.backend.clock_slot_and_timestamp();
        assert_eq!(slot, 2_000);
        assert_eq!(timestamp, MAINNET_GENESIS_UNIX + 2_000 * MS_PER_SLOT / 1000);
        fs::remove_dir_all(&dir).unwrap();
    }

    // A failed send in a world that has dumps names the missing read-only
    // account and points at `Dump::accounts`.
    #[test]
    fn guided_error_names_a_missing_account_when_the_world_has_dumps() {
        let dir = temp_project("guided");
        let address = Pubkey::new_from_array([5; 32]);
        let response = account_response(1, &[(Pubkey::new_from_array([6; 32]), 1, vec![], false)]);
        let mut test = world(&dir, Box::new(RecordedTransport::new(vec![response])));
        test.add(Dump::accounts([address]));

        let bogus = Pubkey::new_from_array([200; 32]);
        let failing = Instruction {
            program_id: crate::SPL_TOKEN_PROGRAM_ID,
            accounts: vec![AccountMeta::new_readonly(bogus, false)],
            data: Vec::new(),
        };
        let outcome = test.simulate(failing);
        assert!(outcome.is_err());
        let hint = outcome
            .hint()
            .expect("a dumped world attaches a guided hint");
        assert!(hint.contains(&bogus.to_string()));
        assert!(hint.contains("Dump::accounts"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // The hint is non-noisy: a world with no dumps attaches none, even on failure.
    #[test]
    fn no_guided_hint_without_dumps() {
        let mut test = Test::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .build()
            .unwrap();
        let failing = Instruction {
            program_id: crate::SPL_TOKEN_PROGRAM_ID,
            accounts: vec![AccountMeta::new_readonly(
                Pubkey::new_from_array([200; 32]),
                false,
            )],
            data: Vec::new(),
        };
        let outcome = test.simulate(failing);
        assert!(outcome.is_err());
        assert!(outcome.hint().is_none());
    }

    #[test]
    fn coherence_warns_only_beyond_one_epoch() {
        assert!(coherence_warning(&[], EPOCH_SLOTS).is_none());
        assert!(coherence_warning(&[100, 200], EPOCH_SLOTS).is_none());
        assert!(coherence_warning(&[10, 10 + EPOCH_SLOTS], EPOCH_SLOTS).is_none());
        let warning = coherence_warning(&[10, 11 + EPOCH_SLOTS], EPOCH_SLOTS).unwrap();
        assert!(warning.contains("refresh_all"));
    }

    // A program dump requests the executable account and its loader-v3
    // programdata together, in one batch, with the right roles.
    #[test]
    fn program_dump_pairs_program_and_programdata() {
        let dir = temp_project("prog-plan");
        let program = Pubkey::new_from_array([7; 32]);
        let programdata = programdata_address(&program);
        let mut test = world(&dir, Box::new(PanicTransport));

        // Only the program id is passed; `dump_plan` expands it to include the
        // programdata, so the shell never derives the programdata address.
        let plan = test
            .dump_plan(
                &dir.to_string_lossy(),
                &[(program, ROLE_PROGRAM)],
                false,
                false,
            )
            .unwrap();
        assert_eq!(
            plan.misses,
            vec![(program, ROLE_PROGRAM), (programdata, ROLE_PROGRAMDATA)]
        );
        let body: Value = serde_json::from_slice(&plan.request_body).unwrap();
        let params = body["params"][0].as_array().unwrap();
        assert_eq!(params[0], program.to_string());
        assert_eq!(params[1], programdata.to_string());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_account_hint_names_the_address() {
        let address = Pubkey::new_from_array([1; 32]);
        assert!(missing_account_hint(Some(address))
            .unwrap()
            .contains(&address.to_string()));
        assert!(missing_account_hint(None).is_none());
    }

    // The one live test: gated on `PARALLAX_LIVE_RPC_TEST=1`, dumps a real
    // well-known mainnet account (the SPL Token program). Skipped by default so
    // the suite stays offline and deterministic.
    #[test]
    fn live_dump_of_a_well_known_account() {
        if std::env::var_os("PARALLAX_LIVE_RPC_TEST").is_none() {
            return;
        }
        let dir = temp_project("live");
        let mut test = Test::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .project_dir(dir.to_string_lossy().into_owned())
            .build()
            .unwrap();
        let [token] = test.add(Dump::accounts([crate::SPL_TOKEN_PROGRAM_ID]));
        let account = test
            .account(token)
            .expect("token program account was dumped");
        assert!(
            account.executable,
            "the token program account is executable"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
