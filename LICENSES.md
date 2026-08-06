# EnerOS workspace licensing

Workspace packages declare one of these SPDX expressions:

- `MIT OR Apache-2.0`: recipients may choose either license.
- `MIT`: the package is available under the MIT License.
- `Apache-2.0`: the package is available under the Apache License 2.0.

The authoritative license texts are:

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE)

Package-specific metadata remains authoritative when it is narrower than the
workspace default. Adding both texts closes the previously inconsistent state
where Cargo metadata offered the MIT option but the repository contained only
the Apache-2.0 text; it does not relicense third-party dependencies.
