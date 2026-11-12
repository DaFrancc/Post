# Deploying Post

A Post server is one pod with two containers (design section 12):

- **Caddy**: terminates TLS on port 443 with a Let's Encrypt certificate and
  forwards to postd. Writes no access log, so client addresses are not
  recorded.
- **postd**: the Post server, listening on localhost inside the pod. Its
  database is encrypted at rest with a key held as a Podman secret.
- **LiveKit** (optional): carries calls' audio and video. The media is
  encrypted end to end with keys only the people in the call derive, so it
  relays frames it cannot read. See "Calls" below.

You need a host with a public IP, a DNS name pointing at it (here
`chat.example.org`), ports 80 and 443 open, and Podman 4.4 or later. For
calls, also ports 7881 (TCP) and 7882 (UDP).

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
cp deploy/Caddyfile deploy/livekit.yaml ~/.config/post/
```

Edit `POST_DOMAIN` in `post-server.container` and `post-caddy.container`, and
the domain in `POST_LIVEKIT_URL`.
**The domain is bound into every account and cannot change later.**

For calls, create the secret postd and LiveKit share:

```sh
printf 'post: %s\n' "$(head -c 36 /dev/urandom | base64 | tr -d '/+=')" | podman secret create post-livekit-keys -
```

To offer no calls instead, delete `post-livekit.container`, the three call
lines in `post-server.container`, and the two call ports in `post.pod`.

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

## Calls

Calls run through LiveKit in the same pod. postd hands out a short-lived
token to anyone who proves they are in a conversation, and LiveKit relays the
call's media, which is encrypted end to end. LiveKit only ever sees a random
name per call for each participant, never who they are.

You set the limits, so calls cannot outgrow your bandwidth:

| Setting (in `post-server.container`) | Default | What it does |
|---|---|---|
| `POST_CALL_MAX_PARTICIPANTS` | `25` | The most people in one call |
| `POST_CALL_MAX_BITRATE_KBPS` | `2500` | The most each person may send. Apps step their quality down to fit |

A rough guide to bandwidth: a voice call uses about 50 kbit/s per person
each way, and video up to the bitrate limit. Each person also receives
everyone else's streams, so a call with N people uses about N × (N − 1)
streams through the server.

## App updates

Apps get updates from their own server, so nobody else sees who runs Post.
The server never updates itself. It hands new versions of the app to the
people who use it, and each person's app still decides when to install.
You decide how much the server does by itself:

| Setting | Default | What it does |
|---|---|---|
| `POST_RELEASES_CHECK` | `true` | Ask GitHub for a new release every hour |
| `POST_RELEASES_DOWNLOAD` | `true` | Download it into `/var/lib/post/releases` and check its signature and files |
| `POST_RELEASES_LIVE_WHILE_RUNNING` | `false` | Offer it to apps as soon as it has downloaded |
| `POST_RELEASES_LIVE_ON_START` | `true` | When the server starts, offer apps the newest downloaded release |

With the defaults, a new release waits on the server until it next restarts.

- To put it live now instead, run `systemctl --user reload post-server`.
  Without systemd, send `postd` the HUP signal
  (`podman kill --signal HUP post-server`, or `kill -HUP <pid>`). A reload is
  you asking, so it does everything whatever the settings say: it re-reads
  the releases folder, asks GitHub, downloads, and puts the newest release
  live. Nobody's connection drops.
- With `POST_RELEASES_LIVE_ON_START=false`, a restart keeps offering the
  release that was live before, and only a reload changes it.
- Set these in `post-server.container` as `Environment=` lines, like
  `POST_DOMAIN`.
- `POST_RELEASES_FROM=owner/repo` fetches from another repository. Leave it
  empty to fetch nothing.
- To skip GitHub altogether, copy a release in by hand: a folder named after
  its version, holding its files, `manifest-<platform>.json` and the `.sig`
  file. Then reload.

## Backups

Back up the `post-data` volume (a SQLite database in WAL mode; stop the pod
or use `sqlite3 .backup` for a consistent copy) and, separately, the
`post-db-key` secret. Losing the server data loses only messages not yet
delivered and the account registry: people keep their own history.

## Alternative: compose

`deploy/compose.yaml` runs the same containers with `podman compose` or
`docker compose`. Put the database key in `deploy/post-db-key` and the calls
secret (`post: <secret>`) in `deploy/post-livekit-keys`, both mode 0600, and
set `POST_DOMAIN`.
