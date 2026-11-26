# Host the downloads page and private sync

[Spindle](../README.md)

## Downloads page

Run `python tools/package-website.py target/website` with Python 3.11 or newer. Upload the contents of `spindle-website.zip` to a dedicated directory in your website, such as `spindle/`. The package includes `index.html` and the app icon. The page links to the release identified by `Cargo.toml` and runs without a database, PHP, or credentials.

Serve the directory over HTTPS and test both the page and the four platform download links. For an update, build the package from the new release and replace the two files in that directory. The installer downloads, checksums, and release notes live on GitHub Releases.

[HostGator file uploads](https://www.hostgator.com/help/article/how-to-upload-a-file-using-the-file-manager)

## Private sync host

Use a Linux host you control or a hosting service that permits encrypted application storage. HostGator shared hosting is suitable for the downloads page. Its acceptable use policy restricts data repositories and sensitive information. [HostGator acceptable use policy](https://www.hostgator.com/tos/acceptable-use-policy)

1. Put the appropriate Spindle Linux binary at `/usr/local/bin/spindle` and make it executable. Install `deploy/spindle-sync.service` at `/etc/systemd/system/spindle-sync.service` as root with mode `0644`.
2. Create `/etc/spindle-sync.env` with mode `0600` and root ownership. Set `SPINDLE_SYNC_ADMIN` to a high-entropy secret generated in your password manager. Keep this file and the token outside the website, repository, and shell history.
3. Start the service with `systemctl enable --now spindle-sync`. Check `http://127.0.0.1:8443/v1/health` on the host. The unit keeps the store under `/var/lib/spindle-sync`, with access limited to the service account and administrators.
4. Terminate HTTPS on a private reverse proxy or VPN endpoint and proxy to `http://127.0.0.1:8443`. Allow access only from approved devices and retain WebSocket support for live sync. Use a dedicated hostname and preserve `/v1/...` paths and request bodies so signature verification succeeds. Check the HTTPS health endpoint from an approved client before registering a workspace.
5. On the first device, use **Settings › Sync › Add server** with the HTTPS address and admin token. Pair other devices from the existing workspace. If you use **Account on your server**, choose a unique passphrase. The account flow stores a verifier and wrapped workspace keys. Devices sign sync requests with their own Ed25519 keys.

Store mode retains encrypted records so a device can catch up after being offline. The host can see routing metadata such as device identifiers and record sizes. Use the host's disk encryption and keep any backups under the same access controls. Users manage pairing and removal from **Settings › Sync › Devices**.

The service supervises the sync process continuously and restarts it on failure. Spindle devices run their scheduled jobs using their existing workspace permissions.
