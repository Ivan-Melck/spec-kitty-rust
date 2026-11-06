# CPython difflib reference — CF73 catalogue diagnosis

`crates/sk-doctrine/src/catalog_diagnosis.rs` contains a Rust port of the
CPython 3.11.15 `difflib.SequenceMatcher` ratio/matching-block algorithm used by
`get_close_matches(..., n=1, cutoff=0.75)`, including autojunk and the tuple
score/text tie-break. Source fixes seq2 to the missing word and assigns each
candidate to seq1: the algorithm is directional, not a symmetric edit metric.

Reference: Python/cpython v3.11.15, `Lib/difflib.py`.
Actual pinned-oracle stdlib bytes:
`0c6afc23568d55b3e9ac914f9c5361e3033e778aa5b58d3cc82835fc5c638679`.
External original/metadata custody `/tmp/sk-cf73-difflib-custody.RFCUSI/difflib.py`
with cp-p/cmp/SHA/stat receipt `.3nNyLD`, parent evidence root
`/tmp/sk-vertical-integration.TwDlpw`. This is provenance custody, not a new
package adoption or a claim that the entire Python module was ported.

Retained complete PSF/Python license:
[cpython-3.11.15-reference-LICENSE.txt](cpython-3.11.15-reference-LICENSE.txt),
SHA256 `3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf`.
The reference difflib module describes matching based on Ratcliff/Obershelp's
1980s pattern matching work, subsequently extended and optimized in Python;
no new Rust dependency, parser fork, unsafe or FFI is introduced here.

This notice is not legal advice, independent supply-chain/safety approval,
release approval, proof for arbitrary adversarial inputs, Python exception/
warning transport equivalence or a performance result. Broader stdlib algorithm
reuse must use this one implementation rather than install a second matcher.
