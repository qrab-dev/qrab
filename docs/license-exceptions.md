# Third-party license exceptions

qrab's code is Apache-2.0. Third-party components must be licensed under the
[CNCF allowed third-party license list][policy] or covered by an approved CNCF
exception ([exceptions.cncf.io](https://exceptions.cncf.io); the source data is
`license-exceptions/exceptions.json` in [cncf/foundation][foundation]).

`cargo deny --locked check licenses` enforces this for Cargo dependencies
using [`deny.toml`](../deny.toml): its `allow` list is the CNCF list, and every
crate outside it has a per-crate exception that names one of the requests below.
`contrib/check-runtime.sh` runs the check when `cargo-deny` is installed.
cargo-deny passes an `OR` expression when any branch is allowed, so dual
licenses such as `Unlicense OR MIT` or `Apache-2.0 OR BSL-1.0` need no
exception. It treats a `WITH` expression as its own license: allowing
Apache-2.0 does not satisfy `Apache-2.0 WITH LLVM-exception`.

The CNCF exception records were checked on 2026-11-18 (data last updated
2026-09-07). None of the licenses below has an approved record that covers
qrab's use, so each has a request ready to file. File them with the
[license exception request template][template] in cncf/foundation, one issue
per license, once qrab is a CNCF project. When one is approved, replace the
`pending:` comment in `deny.toml` with the exception id and issue URL.

Versions are from `Cargo.lock` at the time of writing. Every crate is
unmodified and fetched from crates.io at build time; no third-party Rust source
is stored in the repository.

[policy]: https://github.com/cncf/foundation/blob/main/policies-guidance/allowed-third-party-license-policy.md
[foundation]: https://github.com/cncf/foundation/tree/main/license-exceptions
[template]: https://github.com/cncf/foundation/issues/new?template=license-exception-request.yaml

## Summary

| License | Components | Use | Prior CNCF record |
| --- | --- | --- | --- |
| [Unicode-3.0](#unicode-30) | unicode-ident | build time | Unicode-DFS-2016 blanket, exc-2026-02-18-001 (predecessor license) |

## Unicode-3.0

**Request: blanket exception for Unicode-3.0, or exception for qrab.**

| Component | Version | Upstream URL | License |
| --- | --- | --- | --- |
| unicode-ident | 1.0.27 | https://github.com/dtolnay/unicode-ident | (MIT OR Apache-2.0) AND Unicode-3.0 |

- **How qrab uses it:**
  - unicode-ident supplies the Unicode identifier tables for `proc-macro2`,
    which procedural macros use at compile time.
- **Modified:** no. Fetched from crates.io at build time; nothing is stored in
  the qrab repository.
- **Runtime or build time:** unicode-ident is build time only.
- **Distribution and integration model:** User-Fetched Dependency (fetched by
  Cargo).
- **Structural separation:** Separated Component (separate crates fetched at
  build time). **Communication:** Static Linking. **Data exchange:** Tightly
  Coupled (Rust library calls).
- **Notes for reviewers:** Unicode-3.0 is the OSI-approved successor of
  Unicode-DFS-2016, which the CNCF already approved for all projects
  (exc-2026-02-18-001, [cncf/foundation#1242][textseg]). It is a permissive
  notice-only license.

[textseg]: https://github.com/cncf/foundation/issues/1242
