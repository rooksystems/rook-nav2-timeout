# Vendored upstream sources

Copied unmodified. Verify with `git hash-object <file>` against `git ls-tree` of the upstream commit.

- open-rmf/rmf_traffic @ 39f09e7971c8e666e12c8e9b12199014f631c0bb (2026-06-15), Apache-2.0
- open-rmf/rmf_utils @ 54cc7f6842b88b72bd125d34a8000833dd2b8a38 (2025-07-22), Apache-2.0
- Eigen 5.0.1 (Homebrew), header-only, not vendored; `EIGEN_INCLUDE` in build.sh
- wasi-sdk 33.0 (clang 22.1.0), not vendored; `WASI_SDK_PATH` in build.sh

| file | git blob |
|---|---|
| vendor/rmf_traffic/LICENSE.md | 261eeb9e9f8b2b4b0d119366dda99c6fd7d35c64 |
| vendor/rmf_traffic/include/rmf_traffic/Time.hpp | 76a3aff74709049f5d57e1bcc98ee75b2b190385 |
| vendor/rmf_traffic/include/rmf_traffic/blockade/Moderator.hpp | 0fa4b7b89312694a1d2060b35473c140aaf76bbc |
| vendor/rmf_traffic/include/rmf_traffic/blockade/Status.hpp | 6960aa32e729d8821b165582ed0c6a0cac30f104 |
| vendor/rmf_traffic/include/rmf_traffic/blockade/Writer.hpp | afe0498f5d3130062dcad5174e09aa7b8ea030e2 |
| vendor/rmf_traffic/src/blockade/Constraint.cpp | aac413e5574a6891287bff955bc123b8de305b5e |
| vendor/rmf_traffic/src/blockade/Constraint.hpp | b74a2e2d63a9d6d0c9312843209564332a74fc8d |
| vendor/rmf_traffic/src/blockade/Moderator.cpp | 6f7baa24e42780bd38480414ec2a6ef34d57b06a |
| vendor/rmf_traffic/src/blockade/conflicts.cpp | 4b44ed0da10b7e83990a7f9fdc7f21d4d5bc4ab4 |
| vendor/rmf_traffic/src/blockade/conflicts.hpp | 31caded57615e2d052eb7e296a89c28a9afb7976 |
| vendor/rmf_traffic/src/blockade/geometry.cpp | 049351e153d1e563291d1305d86bbd9edfe0a8c7 |
| vendor/rmf_traffic/src/blockade/geometry.hpp | 0d2a3ecb4a422a74c0222f5daf7216315c8abffd |
| vendor/rmf_utils/LICENSE.md | 261eeb9e9f8b2b4b0d119366dda99c6fd7d35c64 |
| vendor/rmf_utils/include/rmf_utils/Modular.hpp | ca8397c74efe4b0886afb331975eab698c16cb2e |
| vendor/rmf_utils/include/rmf_utils/impl_ptr.hpp | ec678db1bebccec7510fbedb429c85ff7cb2475b |
