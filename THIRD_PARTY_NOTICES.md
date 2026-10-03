# Third-party notices

SaveKeeper itself is licensed under the MIT License (see `LICENSE`).
This file lists third-party data shipped inside the SaveKeeper binaries
(SPEC-05 FR-05-10, FR-05-11). The licenses of the Rust and npm dependencies
are collected in `THIRD_PARTY_LICENSES.html` of the release archive
(SPEC-14 FR-14-08).

## Ludusavi Manifest / PCGamingWiki

Game save data: Ludusavi Manifest (MIT, github.com/mtkennerly/ludusavi-manifest) / PCGamingWiki (CC BY-NC-SA 3.0)

| | |
|---|---|
| Project | Ludusavi Manifest, <https://github.com/mtkennerly/ludusavi-manifest> |
| Author | Matthew T. Kennerly (mtkennerly) |
| Manifest license | MIT, <https://github.com/mtkennerly/ludusavi-manifest/blob/master/LICENSE> |
| Data source | PCGamingWiki, <https://www.pcgamingwiki.com> |
| Data license | Creative Commons Attribution-NonCommercial-ShareAlike 3.0 Unported (CC BY-NC-SA 3.0), <https://creativecommons.org/licenses/by-nc-sa/3.0/>, legal code: <https://creativecommons.org/licenses/by-nc-sa/3.0/legalcode> |
| Snapshot in the repository | `third_party/ludusavi/manifest.yaml` (file `data/manifest.yaml` of the project), date in `third_party/ludusavi/manifest.date` |

> **Before the first release:** re-check the current license statement in the
> README and `LICENSE` of `mtkennerly/ludusavi-manifest` (and the license of the
> PCGamingWiki data it names) and update this section if it changed
> (SPEC-05 T-05-11). The text above follows SPEC-05 FR-05-10 and has not yet
> been verified against the upstream README.

### Changes to the embedded snapshot

The snapshot embedded in SaveKeeper is **not modified**: no entries are added,
removed, filtered or edited. At build time `crates/sk-games/build.rs` only
compresses the YAML file with zstd so that it can be stored in the program file
as a separate resource; SaveKeeper decompresses it unchanged at run time. A
manifest downloaded at run time from `games.manifest_url` is stored in the
`savekeeper-data/cache` folder as received.

### Terms of use of the snapshot (CC BY-NC-SA 3.0)

1. SaveKeeper is distributed **free of charge and non-commercially**.
2. The snapshot is a separate resource inside the binary and keeps its own
   license, CC BY-NC-SA 3.0. ShareAlike applies to these data, not to the
   SaveKeeper code, which stays under the MIT License.
3. The attribution above is shown in the "About" screen of the program, in
   `report.html` of every backup and in this file.

The snapshot is included through the cargo feature `embedded-manifest` of the
`sk-games` crate, which is on by default. A build that must not contain the
data (for example a commercial one) is made with `--no-default-features`; such
a build has no embedded snapshot and only downloads the manifest.
