# Pydantic model repr reference — CF74

The canonical typed body-model view in `body/model_view.rs` follows the pinned
Source's Pydantic 2.13.3 model `__str__`, `__repr__`, declaration-order repr args
and default/type behavior, using the existing Native schema and Kernel Python
repr primitives. It is a bounded Rust implementation, not a Pydantic runtime
installation, schema fork, whole-model/validation port or new Cargo dependency.

Pinned oracle reference bytes retained externally with cp-p/cmp/SHA/stat receipt
`.S9mrmg` in `/tmp/sk-vertical-integration.TwDlpw`:
- `pydantic/main.py`: `35b842cfe92ef300e060b40c062ef93afee1cb075cf0b7b0037a1434867c2554`
- `pydantic/_internal/_repr.py`: `8d07e726ec83c50a5244d846dbd208f4f5fc7b836fdaa599ac4c3696a8a1dd41`

[Complete MIT license](pydantic-2.13.3-reference-LICENSE.txt) retained unchanged:
Copyright (c) 2017 to present Pydantic Services Inc. and individual contributors.
No new dependency/lock/SBOM package row, parser, unsafe or FFI was adopted.
Custody is not legal advice, independent safety/correctness/release approval or
performance certification. Source invalid/admitted values, supported model/type
boundaries and caller-specific evaluation phases remain separate proof duties.
