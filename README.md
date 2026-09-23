# Palmr v4

Palmr is a self-hosted file-sharing application: upload, organize and share files through expiring, revocable links, and receive files from others through Reverse Shares. Palmr v4 is a clean rewrite that ships as a single Rust binary with an embedded web interface, backed by SQLite and local or S3-compatible storage. It is under active development and is not yet ready for production use.

**Palmr v4 is a clean install; v3 data is not upgraded.** There is no v3→v4 migration of any kind: v4 does not import v3 databases, users, files, shares or settings.

Licensed under the [GNU Affero General Public License v3.0 only](LICENSE) (AGPL-3.0-only).
