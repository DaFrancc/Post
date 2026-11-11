# Deploying Post

A Post server is one pod with two containers (design section 12):

- **Caddy**: terminates TLS on port 443 with a Let's Encrypt certificate and
  forwards to postd. Writes no access log, so client addresses are not
  recorded.
- **postd**: the Post server, listening on localhost inside the pod. Its
  database is encrypted at rest with a key held as a Podman secret.

Calls (LiveKit) are added in phase 8.

You need a host with a public IP, a DNS name pointing at it (here
`chat.example.org`), ports 80 and 443 open, and Podman 4.4 or later.

## 1. Build the server image

From the repository root:

```sh
podman build -f deploy/Containerfile -t localhost/postd:latest .
```

## 2. Create the database key

```sh
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | podman secret create post-db-key -
```

Back the key up somewhere safe and separate from the data. Without it the
database cannot be read; with it and a copy of the data, the usernames, device
lists and registration dates can be. Messages are never readable from server
data either way.

## 3. Install the pod (Quadlet, managed by systemd)

```sh
mkdir -p ~/.config/containers/systemd ~/.config/post
cp deploy/quadlet/* ~/.config/containers/systemd/
cp deploy/Caddyfile ~/.config/post/Caddyfile
```

Edit `POST_DOMAIN` in `post-server.container` and `post-caddy.container`.
**The domain is bound into every account and cannot change later.**

To bind ports 80 and 443 as an unprivileged user, allow it once:

```sh
sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80
echo 'net.ipv4.ip_unprivileged_port_start=80' | sudo tee /etc/sysctl.d/50-post.conf
loginctl enable-linger "$USER"   # keep it running after you log out
```

Then start it:

```sh
systemctl --user daemon-reload
systemctl --user start post-pod
```

## 4. Invite people

```sh
podman exec post-server postd invite            # a person
podman exec post-server postd invite --bot      # a bot account
podman exec post-server postd invite --days 1   # expires sooner
```

Each code works once. People register with:

```sh
post keygen ~/.config/post/db.key
post --db-key-file ~/.config/post/db.key register \
    --server wss://chat.example.org/v1/ws --invite CODE --username NAME
```

## 5. Check post-quantum TLS

The design requires the hybrid `X25519MLKEM768` key exchange. With OpenSSL 3.5
or later:

```sh
openssl s_client -connect chat.example.org:443 -groups X25519MLKEM768 </dev/null 2>/dev/null \
  | grep -E 'Negotiated TLS1.3 group|Server Temp Key'
```

It should name `X25519MLKEM768`. The Post client prefers this group, so it
is what clients use whenever the server offers it.

## App updates

Apps get updates from their own server, so nobody else sees who runs Post.
Every hour `postd` checks GitHub (`DaFrancc/Post`) for a newly published
release. It downloads the release into `/var/lib/post/releases` and serves it
only once its signature and files check out. Apps check everything again
themselves before installing.

- `POST_RELEASES_FROM=owner/repo` fetches from another repository. Leave it
  empty to fetch nothing.
- With fetching off, copy a release in by hand instead: a folder named after
  its version, holding its files, `manifest-<platform>.json` and the
  `.sig` file, then restart the pod.

## Backups

Back up the `post-data` volume (a SQLite database in WAL mode; stop the pod
or use `sqlite3 .backup` for a consistent copy) and, separately, the
`post-db-key` secret. Losing the server data loses only messages not yet
delivered and the account registry: people keep their own history.

## Alternative: compose

`deploy/compose.yaml` runs the same two containers with `podman compose` or
`docker compose`. Put the key in `deploy/post-db-key` (mode 0600) and set
`POST_DOMAIN`.
