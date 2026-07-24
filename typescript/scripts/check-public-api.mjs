import { readFile } from "node:fs/promises";

// The published entry points, per package.json `exports`. Only these — and the
// public type graph they pull in — reach a consumer; the internal modules
// (kernel.ts, test.ts) legitimately reference the native transport.
const pkg = JSON.parse(
  await readFile(new URL("../package.json", import.meta.url), "utf8"),
);
const entryDeclarations = Object.values(pkg.exports).map(
  entry => new URL(`../${entry.types.replace(/^\.\//, "")}`, import.meta.url),
);

// Match an actual module specifier for a private transport (a quoted `koffi`,
// `litesvm`, or a `./internal/kernel` path in an import/export), not the bare
// word, which legitimately appears in prose and doc comments.
const forbidden = [
  { label: "the koffi FFI runtime", pattern: /["']koffi(?:\/[^"']*)?["']/ },
  { label: "the LiteSVM backend", pattern: /["']litesvm(?:\/[^"']*)?["']/ },
  { label: "the internal kernel", pattern: /["'][^"']*internal\/kernel(?:\.js)?["']/ },
];

for (const declaration of entryDeclarations) {
  const source = await readFile(declaration, "utf8");
  for (const { label, pattern } of forbidden) {
    if (pattern.test(source)) {
      throw new Error(
        `${declaration.pathname} exposes ${label} in the public API`,
      );
    }
  }
}
