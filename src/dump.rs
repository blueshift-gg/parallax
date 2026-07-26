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

/// Magic tag beginning every dump file: "parallax dump".
const DUMP_MAGIC: [u8; 4] = *b"PLXD";

/// Dump file format version. A file written by a newer format is rejected with
/// a "re-dump" message rather than silently misread.
const DUMP_FORMAT_VERSION: u16 = 1;

/// Length of the hand-framed header (magic + little-endian version) preceding
/// the wincode body. Kept outside wincode so a bad magic or version is caught
/// before decoding — never a huge allocation from a garbage length prefix.
const DUMP_HEADER_LEN: usize = 6;

/// File extension for a dump file inside the store or shared elsewhere.
const DUMP_EXT: &str = "dump";

/// Directory (next to the project manifest) that holds the committed store.
const STORE_DIR: &str = ".parallax";

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
}

// ---------------------------------------------------------------------------
// Dump file format (wincode) — shared by the `.parallax/` store and `Load`
// ---------------------------------------------------------------------------

/// The wincode body following the [`DUMP_MAGIC`] + version header.
#[derive(wincode::SchemaRead, wincode::SchemaWrite)]
struct DumpBodyWire {
    slot: u64,
    entries: Vec<DumpEntryWire>,
}

/// One account (or program, whose `data` is its ELF) in a dump file.
#[derive(wincode::SchemaRead, wincode::SchemaWrite)]
struct DumpEntryWire {
    address: [u8; 32],
    lamports: u64,
    owner: [u8; 32],
    executable: u8,
    role: u8,
    data: Vec<u8>,
}

/// One decoded dump entry.
#[derive(Clone, Debug)]
pub(crate) struct StoredEntry {
    address: Pubkey,
    lamports: u64,
    owner: Pubkey,
    executable: bool,
    role: Role,
    data: Vec<u8>,
}

/// A decoded dump file: the observed slot and its entries.
#[derive(Debug)]
pub(crate) struct DumpFile {
    slot: u64,
    entries: Vec<StoredEntry>,
}

/// Serialize a dump file: [`DUMP_MAGIC`] + version + wincode body. This is the
/// one format for both a store file and a `Load` input, so any file the store
/// writes is directly loadable and shareable.
fn write_dump_file(slot: u64, entries: &[StoredEntry]) -> Vec<u8> {
    let body = DumpBodyWire {
        slot,
        entries: entries
            .iter()
            .map(|entry| DumpEntryWire {
                address: entry.address.to_bytes(),
                lamports: entry.lamports,
                owner: entry.owner.to_bytes(),
                executable: u8::from(entry.executable),
                role: entry.role.code(),
                data: entry.data.clone(),
            })
            .collect(),
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&DUMP_MAGIC);
    bytes.extend_from_slice(&DUMP_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&wincode::serialize(&body).expect("dump body serializes"));
    bytes
}

/// Parse a dump file, with actionable errors that name `path` and the expected
/// format version. Shared by the `Load` path and the `.parallax/` store, so the
/// message names the file rather than either caller.
fn read_dump_file(path: &Path, bytes: &[u8]) -> Result<DumpFile, String> {
    let display = path.display();
    if bytes.len() < DUMP_HEADER_LEN {
        return Err(format!(
            "{display} is truncated — not a parallax dump file (expected format v{DUMP_FORMAT_VERSION})"
        ));
    }
    if bytes[0..4] != DUMP_MAGIC {
        return Err(format!(
            "{display} is not a parallax dump file (bad magic; expected format v{DUMP_FORMAT_VERSION})"
        ));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != DUMP_FORMAT_VERSION {
        return Err(format!(
            "{display} is dump format v{version}, but this build reads v{DUMP_FORMAT_VERSION}; re-dump the file"
        ));
    }
    let body: DumpBodyWire = wincode::deserialize_exact(&bytes[DUMP_HEADER_LEN..])
        .map_err(|error| format!("{display} is corrupt or truncated: {error:?}"))?;
    let mut entries = Vec::with_capacity(body.entries.len());
    for entry in body.entries {
        entries.push(StoredEntry {
            address: Pubkey::new_from_array(entry.address),
            lamports: entry.lamports,
            owner: Pubkey::new_from_array(entry.owner),
            executable: entry.executable != 0,
            role: Role::from_code(entry.role)?,
            data: entry.data,
        });
    }
    Ok(DumpFile {
        slot: body.slot,
        entries,
    })
}

// ---------------------------------------------------------------------------
// Store (a directory of per-primary dump files)
// ---------------------------------------------------------------------------

/// The committed `.parallax/` store: a directory of `<primary-address>.dump`
/// files, each a self-contained [`DumpFile`]. Because a store file *is* a dump
/// file, users share a dump by copying the file out of `.parallax/` and
/// [`Load`](crate::fixture::Load)-ing it by path. Only the core reads or writes
/// the store, so the format has exactly one implementation.
pub(crate) struct DumpStore {
    dir: PathBuf,
}

impl DumpStore {
    pub(crate) fn open(project_dir: &Path) -> Self {
        Self {
            dir: project_dir.join(STORE_DIR),
        }
    }

    fn file_path(&self, primary: &Pubkey) -> PathBuf {
        self.dir.join(format!("{primary}.{DUMP_EXT}"))
    }

    /// The stored dump file for `primary`, if present.
    fn get(&self, primary: &Pubkey) -> Result<Option<DumpFile>, String> {
        let path = self.file_path(primary);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(read_dump_file(&path, &bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("dump: could not read {}: {error}", path.display())),
        }
    }

    /// Write a one-primary dump file with `entries` observed at `slot`.
    fn put(&self, primary: &Pubkey, slot: u64, entries: &[StoredEntry]) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("dump: could not create {}: {error}", self.dir.display()))?;
        let path = self.file_path(primary);
        fs::write(&path, write_dump_file(slot, entries))
            .map_err(|error| format!("dump: could not write {}: {error}", path.display()))
    }

    /// Every stored file's primary address and unit kind (Account or Program),
    /// for a refresh that re-fetches everything. Sorted for determinism.
    fn stored_units(&self) -> Result<Vec<(Pubkey, Role)>, String> {
        let read = match fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "dump: could not read {}: {error}",
                    self.dir.display()
                ))
            }
        };
        let mut units = Vec::new();
        for entry in read {
            let path = entry
                .map_err(|error| format!("dump: could not read {}: {error}", self.dir.display()))?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(DUMP_EXT) {
                continue;
            }
            let Some(primary) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| Pubkey::from_str(stem).ok())
            else {
                continue;
            };
            let bytes = fs::read(&path)
                .map_err(|error| format!("dump: could not read {}: {error}", path.display()))?;
            let file = read_dump_file(&path, &bytes)?;
            let kind = if file.entries.iter().any(|entry| entry.role == Role::Program) {
                Role::Program
            } else {
                Role::Account
            };
            units.push((primary, kind));
        }
        units.sort_by_key(|(primary, _)| primary.to_bytes());
        Ok(units)
    }
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

/// Extract a program's ELF from its fetched account: the programdata body for
/// the upgradeable loader, or the program account itself for the older loaders.
fn program_elf(
    program_id: &Pubkey,
    program: &FetchedAccount,
    fetched: &BTreeMap<Pubkey, Option<FetchedAccount>>,
) -> Result<Vec<u8>, String> {
    if program.owner != bpf_loader_upgradeable::ID {
        return Ok(program.data.clone());
    }
    let programdata = programdata_address(program_id);
    let source = fetched
        .get(&programdata)
        .and_then(Option::as_ref)
        .ok_or_else(|| {
            format!(
                "dump: program {program_id} is loader-v3 but its programdata {programdata} was \
                 not returned"
            )
        })?;
    Ok(source
        .data
        .get(PROGRAMDATA_HEADER_LEN..)
        .ok_or_else(|| format!("dump: programdata {programdata} is too small to hold an ELF"))?
        .to_vec())
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
    /// returns the batched request body for the misses (empty when the store
    /// already covers every target). A `Program` miss expands to fetch its
    /// loader-v3 programdata too, so the derivation stays core-side. When
    /// `refresh` is set, every stored file is a target and all are re-fetched.
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
        let store = DumpStore::open(Path::new(project_dir));
        let units: Vec<(Pubkey, Role)> = if refresh {
            store.stored_units()?
        } else {
            targets
                .iter()
                .map(|(address, code)| Role::from_code(*code).map(|role| (*address, role)))
                .collect::<Result<_, _>>()?
        };

        let mut fetch: Vec<(Pubkey, Role)> = Vec::new();
        let mut hit_slot: Option<u64> = None;
        for (primary, kind) in units {
            if !refresh {
                if let Some(file) = store.get(&primary)? {
                    self.install_entries(&file.entries)?;
                    self.record_dumped(&file.entries, file.slot);
                    hit_slot = Some(hit_slot.map_or(file.slot, |slot| slot.max(file.slot)));
                    continue;
                }
            }
            match kind {
                Role::Account => fetch.push((primary, Role::Account)),
                Role::Program => {
                    fetch.push((primary, Role::Program));
                    fetch.push((programdata_address(&primary), Role::ProgramData));
                }
                Role::ProgramData => {
                    return Err("dump: programdata is not a valid dump target".into())
                }
            }
        }

        if fetch.is_empty() {
            self.finish_dump(sync_clock, hit_slot);
            return Ok(DumpPlan {
                request_body: Vec::new(),
                misses: Vec::new(),
            });
        }

        let request_body = build_request(
            &fetch
                .iter()
                .map(|(address, _)| *address)
                .collect::<Vec<_>>(),
        );
        Ok(DumpPlan {
            request_body,
            misses: fetch
                .into_iter()
                .map(|(address, role)| (address, role.code()))
                .collect(),
        })
    }

    /// Phase two of a dump. Parses the RPC response for the `misses` returned by
    /// [`Self::dump_plan`], writes one store file per primary (a program's file
    /// carries its extracted ELF as a single `Program` entry), installs the
    /// accounts, and applies `sync_clock` from the observed slot.
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
        let store = DumpStore::open(Path::new(project_dir));
        let addresses: Vec<Pubkey> = misses.iter().map(|(address, _)| *address).collect();
        let (slot, values) = parse_response(&addresses, response_body)?;

        // Index fetched values by address so a program reaches its programdata.
        let mut fetched: BTreeMap<Pubkey, Option<FetchedAccount>> = BTreeMap::new();
        for (address, value) in addresses.iter().zip(values) {
            fetched.insert(*address, value);
        }

        let mut installed: Vec<StoredEntry> = Vec::new();
        for (address, code) in misses {
            let entry = match Role::from_code(*code)? {
                Role::Account => match fetched.get(address).and_then(Option::as_ref) {
                    Some(account) => StoredEntry {
                        address: *address,
                        lamports: account.lamports,
                        owner: account.owner,
                        executable: account.executable,
                        role: Role::Account,
                        data: account.data.clone(),
                    },
                    None => {
                        eprintln!(
                            "parallax dump: account {address} does not exist on chain; \
                             skipped (dump only real addresses)"
                        );
                        continue;
                    }
                },
                Role::Program => {
                    let Some(program) = fetched.get(address).and_then(Option::as_ref) else {
                        eprintln!(
                            "parallax dump: program {address} does not exist on chain; skipped"
                        );
                        continue;
                    };
                    StoredEntry {
                        address: *address,
                        lamports: program.lamports,
                        owner: program.owner,
                        executable: true,
                        role: Role::Program,
                        data: program_elf(address, program, &fetched)?,
                    }
                }
                // A programdata address is consumed by its program above.
                Role::ProgramData => continue,
            };
            store.put(address, slot, std::slice::from_ref(&entry))?;
            installed.push(entry);
        }

        self.install_entries(&installed)?;
        self.record_dumped(&installed, slot);
        eprintln!(
            "parallax: dumped {} account(s) @ slot {slot}",
            installed.len()
        );
        self.finish_dump(sync_clock, Some(slot));
        Ok(())
    }

    /// Install every entry of the dump file at `path` — no store, no network.
    /// Returns the installed primary addresses (a program's id when
    /// `expect_program`). Backs [`Load`](crate::fixture::Load).
    ///
    /// Internal FFI support; not a stable API.
    #[doc(hidden)]
    pub fn load_dump(&mut self, path: &str, expect_program: bool) -> Result<Vec<Pubkey>, String> {
        let path = Path::new(path);
        let bytes = fs::read(path)
            .map_err(|error| format!("load: could not read {}: {error}", path.display()))?;
        let file = read_dump_file(path, &bytes)?;
        let program = file
            .entries
            .iter()
            .find(|entry| entry.role == Role::Program);
        if expect_program && program.is_none() {
            return Err(format!(
                "load: {} contains no program — use Load::accounts for an account file",
                path.display()
            ));
        }
        let addresses = if expect_program {
            vec![
                program
                    .expect("program present after the check above")
                    .address,
            ]
        } else {
            file.entries
                .iter()
                .filter(|entry| entry.role != Role::ProgramData)
                .map(|entry| entry.address)
                .collect()
        };
        self.install_entries(&file.entries)?;
        self.record_dumped(&file.entries, file.slot);
        self.finish_dump(false, None);
        Ok(addresses)
    }

    /// Install resolved dump entries. Accounts are set directly; a program is
    /// made executable by loading its stored ELF under the loader that owns it.
    fn install_entries(&mut self, entries: &[StoredEntry]) -> Result<(), String> {
        for entry in entries {
            match entry.role {
                Role::Account => self.backend.set_account(Account {
                    address: entry.address,
                    lamports: entry.lamports,
                    data: entry.data.clone(),
                    owner: entry.owner,
                    executable: entry.executable,
                }),
                Role::Program => {
                    self.backend.load_program_with_loader(
                        &entry.address,
                        &entry.data,
                        entry.owner,
                    )?;
                }
                // Never stored; a programdata account is folded into its program.
                Role::ProgramData => {}
            }
        }
        Ok(())
    }

    /// Record installed dump entries for coherence tracking and guided errors.
    fn record_dumped(&mut self, entries: &[StoredEntry], slot: u64) {
        for entry in entries {
            self.dumped_addresses.push(entry.address);
            self.dumped_slots.push(slot);
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

    /// Native path for `Load`: install a dump file by path, panicking with an
    /// actionable message on failure, as the rest of fixture setup does.
    pub(crate) fn load_file_native(&mut self, path: &Path, expect_program: bool) -> Vec<Pubkey> {
        let path_str = path
            .to_str()
            .unwrap_or_else(|| panic!("load: path is not valid UTF-8: {}", path.display()));
        self.load_dump(path_str, expect_program)
            .unwrap_or_else(|error| panic!("{error}"))
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
        // With `refresh`, the resolved set is every primary in the misses — the
        // programdata addresses a program miss expands to are not primaries.
        // Otherwise it is the requested targets.
        let resolved: Vec<Pubkey> = if refresh {
            plan.misses
                .iter()
                .filter(|(_, role)| *role != ROLE_PROGRAMDATA)
                .map(|(address, _)| *address)
                .collect()
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
            "missing account {address}: if it exists on mainnet, add it to your \
             dump accounts fixture"
        )
    })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            fixture::{Dump, Load},
            AccountMeta, Instruction, Test,
        },
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

    fn entry(address: Pubkey, role: Role, data: Vec<u8>) -> StoredEntry {
        StoredEntry {
            address,
            lamports: 1000,
            owner: Pubkey::new_from_array([3; 32]),
            executable: role == Role::Program,
            role,
            data,
        }
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = temp_project("store");
        let store = DumpStore::open(&dir);
        let address = Pubkey::new_from_array([7; 32]);
        store
            .put(
                &address,
                42,
                &[entry(address, Role::Account, vec![4, 5, 6])],
            )
            .unwrap();

        let file = store.get(&address).unwrap().unwrap();
        assert_eq!(file.slot, 42);
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].data, vec![4, 5, 6]);
        assert_eq!(file.entries[0].role, Role::Account);
        // Each entry is its own `<address>.dump` file under `.parallax/`.
        assert!(dir
            .join(STORE_DIR)
            .join(format!("{address}.{DUMP_EXT}"))
            .is_file());
        // A missing primary reads back as absent.
        assert!(store
            .get(&Pubkey::new_from_array([99; 32]))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    // The store file IS a `Load` input: a file written by the store round-trips
    // through the wincode format, and bad magic / a future version / truncation
    // each produce an actionable, file-named error.
    #[test]
    fn dump_file_round_trips_and_rejects_bad_files() {
        let address = Pubkey::new_from_array([7; 32]);
        let bytes = write_dump_file(1_234, &[entry(address, Role::Account, vec![1, 2, 3])]);
        let path = Path::new("in-memory.dump");
        let file = read_dump_file(path, &bytes).unwrap();
        assert_eq!(file.slot, 1_234);
        assert_eq!(file.entries[0].address, address);

        // Bad magic.
        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'X';
        assert!(read_dump_file(path, &wrong_magic)
            .unwrap_err()
            .contains("not a parallax dump file"));

        // A future format version tells the user to re-dump.
        let mut future = bytes.clone();
        future[4] = 0xFF;
        let error = read_dump_file(path, &future).unwrap_err();
        assert!(error.contains("re-dump"));

        // Truncation is rejected, naming the file.
        assert!(read_dump_file(path, &bytes[..3])
            .unwrap_err()
            .contains("in-memory.dump"));
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
        assert!(hint.contains("dump accounts fixture"));
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

    // A store file IS a Load input: Dump writes `<address>.dump`, and a fresh,
    // network-free world Loads that exact file by path to the identical state.
    #[test]
    fn load_installs_a_dumped_file_offline() {
        let dir = temp_project("load");
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([6; 32]);
        let data = vec![9, 8, 7];
        let response = account_response(1_500, &[(owner, 42, data.clone(), false)]);

        {
            let mut test = world(&dir, Box::new(RecordedTransport::new(vec![response])));
            test.add(Dump::accounts([address]));
        }
        let file = dir.join(STORE_DIR).join(format!("{address}.{DUMP_EXT}"));
        assert!(file.is_file());

        let mut test = Test::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .transport(Box::new(PanicTransport))
            .build()
            .unwrap();
        let loaded = test.add(Load::accounts(file));
        assert_eq!(loaded, vec![address]);
        let account = test.account(address).unwrap();
        assert_eq!(account.owner, owner);
        assert_eq!(account.lamports, 42);
        assert_eq!(account.data, data);
        fs::remove_dir_all(&dir).unwrap();
    }

    // Load::program refuses a file that carries no program.
    #[test]
    fn load_program_rejects_an_account_file() {
        let dir = temp_project("load-prog-err");
        let address = Pubkey::new_from_array([5; 32]);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{address}.{DUMP_EXT}"));
        fs::write(
            &path,
            write_dump_file(1, &[entry(address, Role::Account, vec![1])]),
        )
        .unwrap();

        let mut test = Test::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .build()
            .unwrap();
        let error = test.load_dump(path.to_str().unwrap(), true).unwrap_err();
        assert!(error.contains("contains no program"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // A program dump file round-trips: the stored `Program` entry carries the
    // ELF, so a warm hit (and `Load`) can reload it without the programdata.
    #[test]
    fn program_entry_round_trips_with_its_elf() {
        let program = Pubkey::new_from_array([7; 32]);
        let elf = vec![0x7f, b'E', b'L', b'F', 1, 2, 3];
        let mut program_entry = entry(program, Role::Program, elf.clone());
        program_entry.owner = bpf_loader_upgradeable::ID;
        let bytes = write_dump_file(9, &[program_entry]);
        let file = read_dump_file(Path::new("p.dump"), &bytes).unwrap();
        assert_eq!(file.entries[0].role, Role::Program);
        assert_eq!(file.entries[0].data, elf);
        assert!(file.entries[0].executable);
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
