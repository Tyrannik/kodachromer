# Third-party software

Remjet is distributed under the GNU GPL v3 or later (see LICENSE). The Windows binaries
statically include the open-source Rust crates listed below, each under its own license as
declared by its authors. All are compatible with distribution under the GPL v3.

Notable components:

- **rawler** (LGPL-2.1) - raw/DNG decoding, from the dnglab project: https://github.com/dnglab/dnglab.
  Under LGPL-2.1 section 3 it may be distributed under the GPL; its complete source is available
  from crates.io and the project above, and Remjet's full source is in this repository.
- **egui / eframe** (MIT OR Apache-2.0) - user interface. Bundled fonts: Ubuntu Font Licence 1.0
  and SIL Open Font License 1.1 (epaint_default_fonts).
- **image**, **jpeg-encoder**, **rayon**, **serde**, **toml**, **rfd**, **wgpu** - MIT and/or Apache-2.0.

Film data: the Kodachrome 64 characteristic and spectral dye-density curves in
presets/kodachrome64.toml were digitised from Kodak publication E-55 (06/2009) for emulation.
The Ektachrome E100 data in presets/ektachrome-e100.toml were digitised from Kodak publication
E-4000, and the Portra 400 and Endura Premier paper data in presets/portra400.toml from Kodak
publications E-4050 and E-4070. The Ektar 100, Gold 200 and Ultramax 400 data in
presets/ektar100.toml, presets/gold200.toml and presets/ultramax400.toml were digitised from
Kodak publications E-4046, E-7022 and E-7023 (their paper data is the E-4070 set). The Tri-X 400 data in presets/trix400.toml were digitised
from Kodak publication F-4017. The colour films' push and pull models are fitted to Portra 800
curves digitised from Kodak publication E-4040. Colorimetric tables in crates/remjet-core/src/cie.rs are from
CIE 015:2018 and ISO 5-3:1995. Kodachrome, Ektachrome, Portra, Ektar, Gold, Ultramax, Tri-X, Endura and Kodak are trademarks of Eastman Kodak Company; this project is not affiliated
with or endorsed by Kodak or Leica.

## Crates (263)

| Crate | Version | License |
|---|---|---|
| accesskit | 0.25.2 | MIT OR Apache-2.0 |
| accesskit_consumer | 0.39.2 | MIT OR Apache-2.0 |
| accesskit_windows | 0.35.2 | MIT OR Apache-2.0 |
| accesskit_winit | 0.34.2 | Apache-2.0 |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| adler32 | 1.2.0 | Zlib |
| ahash | 0.8.12 | MIT OR Apache-2.0 |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| alloc-no-stdlib | 2.0.4 | BSD-3-Clause |
| alloc-stdlib | 0.2.4 | BSD-3-Clause |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| arboard | 3.7.0 | MIT OR Apache-2.0 |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 |
| ash | 0.38.0+1.3.281 | MIT OR Apache-2.0 |
| backtrace | 0.3.77 | MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| bit-set | 0.10.0 | Apache-2.0 OR MIT |
| bitstream-io | 4.10.0 | MIT/Apache-2.0 |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT |
| brotli-decompressor | 5.0.3 | BSD-3-Clause/MIT |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT |
| byteorder | 1.5.0 | Unlicense OR MIT |
| byteorder-lite | 0.1.0 | Unlicense OR MIT |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| clipboard-win | 5.4.3 | BSL-1.0 |
| codespan-reporting | 0.13.1 | Apache-2.0 |
| color | 0.3.3 | Apache-2.0 OR MIT |
| crc32fast | 1.5.5 | MIT OR Apache-2.0 |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| cursor-icon | 1.2.0 | MIT OR Apache-2.0 OR Zlib |
| dary_heap | 0.3.9 | MIT OR Apache-2.0 |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 |
| document-features | 0.2.12 | MIT OR Apache-2.0 |
| dpi | 0.1.2 | Apache-2.0 AND MIT |
| ecolor | 0.37.0 | MIT OR Apache-2.0 |
| eframe | 0.37.0 | MIT OR Apache-2.0 |
| egui | 0.37.0 | MIT OR Apache-2.0 |
| egui_glow | 0.37.0 | MIT OR Apache-2.0 |
| egui_system_font_provider | 0.37.0 | MIT OR Apache-2.0 |
| egui-wgpu | 0.37.0 | MIT OR Apache-2.0 |
| egui-winit | 0.37.0 | MIT OR Apache-2.0 |
| either | 1.19.0 | MIT OR Apache-2.0 |
| emath | 0.37.0 | MIT OR Apache-2.0 |
| enumn | 0.1.14 | MIT OR Apache-2.0 |
| epaint | 0.37.0 | MIT OR Apache-2.0 |
| epaint_default_fonts | 0.37.0 | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| error-code | 3.4.0 | BSL-1.0 |
| euclid | 0.22.14 | MIT OR Apache-2.0 |
| fax | 0.2.7 | MIT |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| fearless_simd | 0.4.1 | Apache-2.0 OR MIT |
| flate2 | 1.1.11 | MIT OR Apache-2.0 |
| foldhash | 0.2.0 | Zlib |
| fontique | 0.11.1 | Apache-2.0 OR MIT |
| font-types | 0.12.7 | MIT OR Apache-2.0 |
| form_urlencoded | 1.2.3 | MIT OR Apache-2.0 |
| getrandom | 0.3.4 | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| glifo | 0.3.0 | Apache-2.0 OR MIT |
| glow | 0.17.0 | MIT OR Apache-2.0 OR Zlib |
| glutin | 0.32.3 | Apache-2.0 |
| glutin_egl_sys | 0.7.1 | Apache-2.0 |
| glutin_wgl_sys | 0.6.1 | Apache-2.0 |
| glutin-winit | 0.5.0 | MIT |
| gpu-allocator | 0.28.0 | MIT OR Apache-2.0 |
| guillotiere | 0.7.0 | MIT/Apache-2.0 |
| half | 2.7.1 | MIT OR Apache-2.0 |
| harfrust | 0.12.0 | MIT |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| hex | 0.4.3 | MIT OR Apache-2.0 |
| icu_collections | 2.4.0 | Unicode-3.0 |
| icu_locale_core | 2.4.0 | Unicode-3.0 |
| icu_normalizer | 2.4.0 | Unicode-3.0 |
| icu_normalizer_data | 2.4.0 | Unicode-3.0 |
| icu_properties | 2.4.0 | Unicode-3.0 |
| icu_properties_data | 2.4.0 | Unicode-3.0 |
| icu_provider | 2.4.0 | Unicode-3.0 |
| idna | 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT |
| image | 0.25.10 | MIT OR Apache-2.0 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| itertools | 0.14.0 | MIT OR Apache-2.0 |
| itertools | 0.15.0 | MIT OR Apache-2.0 |
| itoa | 1.0.19 | MIT OR Apache-2.0 |
| jpeg-encoder | 0.7.1 | (MIT OR Apache-2.0) AND IJG |
| jxl-bitstream | 1.1.0 | MIT OR Apache-2.0 |
| jxl-coding | 1.0.1 | MIT OR Apache-2.0 |
| jxl-color | 0.11.0 | MIT OR Apache-2.0 |
| jxl-frame | 0.13.3 | MIT OR Apache-2.0 |
| jxl-grid | 0.6.2 | MIT OR Apache-2.0 |
| jxl-image | 0.13.0 | MIT OR Apache-2.0 |
| jxl-jbr | 0.2.1 | MIT OR Apache-2.0 |
| jxl-modular | 0.11.3 | MIT OR Apache-2.0 |
| jxl-oxide | 0.12.6 | MIT OR Apache-2.0 |
| jxl-oxide-common | 1.0.0 | MIT OR Apache-2.0 |
| jxl-render | 0.12.4 | MIT OR Apache-2.0 |
| jxl-threadpool | 1.0.0 | MIT OR Apache-2.0 |
| jxl-vardct | 0.11.1 | MIT OR Apache-2.0 |
| khronos-egl | 6.0.0 | MIT/Apache-2.0 |
| kurbo | 0.13.1 | Apache-2.0 OR MIT |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| libflate | 2.3.2 | MIT |
| libflate_lz77 | 2.3.0 | MIT |
| libloading | 0.8.9 | ISC |
| libm | 0.2.16 | MIT |
| linebender_resource_handle | 0.1.1 | Apache-2.0 OR MIT |
| litemap | 0.8.3 | Unicode-3.0 |
| litrs | 1.0.1 | MIT OR Apache-2.0 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.35 | MIT OR Apache-2.0 |
| md5 | 0.8.1 | Apache-2.0 OR MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| memmap2 | 0.9.12 | MIT OR Apache-2.0 |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| moxcms | 0.8.1 | BSD-3-Clause OR Apache-2.0 |
| multiversion | 0.8.1 | MIT OR Apache-2.0 |
| multiversion-macros | 0.8.1 | MIT OR Apache-2.0 |
| naga | 30.0.2 | MIT OR Apache-2.0 |
| naga-types | 30.0.2 | MIT OR Apache-2.0 |
| no_std_io2 | 0.9.4 | Apache-2.0 OR MIT |
| nohash-hasher | 0.2.0 | Apache-2.0 OR MIT |
| num | 0.4.3 | MIT OR Apache-2.0 |
| num_enum | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 |
| num_enum_derive | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 |
| num-complex | 0.4.6 | MIT OR Apache-2.0 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| num-iter | 0.1.46 | MIT OR Apache-2.0 |
| num-rational | 0.4.2 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| ordered-float | 5.5.1 | MIT |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| parlance | 0.1.1 | Apache-2.0 OR MIT |
| peniko | 0.6.1 | Apache-2.0 OR MIT |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.18 | Apache-2.0 OR MIT |
| png | 0.18.1 | MIT OR Apache-2.0 |
| poll-promise | 0.3.0 | MIT OR Apache-2.0 |
| pollster | 1.0.3 | MIT OR Apache-2.0 |
| polycool | 0.4.0 | MIT OR Apache-2.0 |
| potential_utf | 0.1.6 | Unicode-3.0 |
| presser | 0.3.1 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 |
| profiling | 1.0.18 | MIT OR Apache-2.0 |
| pxfm | 0.1.30 | BSD-3-Clause OR Apache-2.0 |
| quick-error | 2.0.1 | MIT/Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| range-alloc | 0.1.5 | MIT OR Apache-2.0 |
| rawler | 0.8.0 | LGPL-2.1 |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| rayon | 1.12.0 | MIT OR Apache-2.0 |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 |
| read-fonts | 0.41.0 | MIT OR Apache-2.0 |
| renderdoc-sys | 1.1.0 | MIT OR Apache-2.0 |
| rfd | 0.18.0 | MIT |
| rle-decode-fast | 1.0.3 | MIT OR Apache-2.0 |
| rustc-demangle | 0.1.28 | MIT/Apache-2.0 |
| rustc-hash | 1.1.0 | Apache-2.0/MIT |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| self_cell | 1.3.0 | Apache-2.0 OR GPL-2.0-only |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 |
| serde_spanned | 1.1.3 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| skrifa | 0.44.0 | MIT OR Apache-2.0 |
| smallvec | 1.16.6 | MIT OR Apache-2.0 |
| smol_str | 0.2.2 | MIT OR Apache-2.0 |
| spirv | 0.4.0+sdk-1.4.341.0 | Apache-2.0 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.10 | MIT OR Apache-2.0 |
| synstructure | 0.14.0 | MIT |
| sys-locale | 0.3.2 | MIT OR Apache-2.0 |
| target-features | 0.1.6 | MIT OR Apache-2.0 |
| termcolor | 1.4.1 | Unlicense OR MIT |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| tiff | 0.11.3 | MIT |
| tinystr | 0.8.5 | Unicode-3.0 |
| toml | 0.8.23 | MIT OR Apache-2.0 |
| toml | 1.2.2+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime | 0.6.11 | MIT OR Apache-2.0 |
| toml_datetime | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 |
| toml_edit | 0.25.20+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_parser | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_write | 0.1.2 | MIT OR Apache-2.0 |
| toml_writer | 1.2.0+spec-1.1.0 | MIT OR Apache-2.0 |
| tracing | 0.1.44 | MIT |
| tracing-attributes | 0.1.31 | MIT |
| tracing-core | 0.1.36 | MIT |
| type-map | 0.5.1 | MIT/Apache-2.0 |
| unicode-general-category | 1.1.0 | Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-properties | 0.1.5 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-script | 0.5.9 | MIT OR Apache-2.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| unicode-width | 0.2.3 | MIT OR Apache-2.0 |
| url | 2.5.9 | MIT OR Apache-2.0 |
| utf_types | 1.0.0 | Apache-2.0 OR MIT |
| utf8_iter | 1.1.0 | Apache-2.0 OR MIT |
| uuid | 1.29.3 | Apache-2.0 OR MIT |
| vello_common | 0.2.0 | Apache-2.0 OR MIT |
| vello_cpu | 0.2.0 | Apache-2.0 OR MIT |
| webbrowser | 1.2.7 | MIT OR Apache-2.0 |
| web-time | 1.1.0 | MIT OR Apache-2.0 |
| weezl | 0.1.12 | MIT OR Apache-2.0 |
| wgpu | 30.0.2 | MIT OR Apache-2.0 |
| wgpu-core | 30.0.2 | MIT OR Apache-2.0 |
| wgpu-core-deps-windows-linux-android | 30.0.2 | MIT OR Apache-2.0 |
| wgpu-hal | 30.0.2 | MIT OR Apache-2.0 |
| wgpu-naga-bridge | 30.0.2 | MIT OR Apache-2.0 |
| wgpu-types | 30.0.2 | MIT OR Apache-2.0 |
| winapi-util | 0.1.11 | Unlicense OR MIT |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| winit | 0.30.13 | Apache-2.0 |
| winnow | 0.7.15 | MIT |
| winnow | 1.0.5 | MIT |
| writeable | 0.6.4 | Unicode-3.0 |
| yoke | 0.8.4 | Unicode-3.0 |
| yoke-derive | 0.8.4 | Unicode-3.0 |
| zerocopy | 0.8.64 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.64 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 |
| zerotrie | 0.2.5 | Unicode-3.0 |
| zerovec | 0.11.9 | Unicode-3.0 |
| zerovec-derive | 0.11.7 | Unicode-3.0 |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.23 | MIT |
| zune-core | 0.5.3 | MIT OR Apache-2.0 OR Zlib |
| zune-jpeg | 0.5.15 | MIT OR Apache-2.0 OR Zlib |
