# Running Bomb Code on a server

## One person

    bombd up --data ~/.bombd --listen 0.0.0.0:7443 --public <your-server-address>:7443

Prints a pairing link. Paste it into Bomb Code → Settings → Servers on your Mac. Open TCP port 7443.
Everything runs as you; your projects live in `~/projects`.

## Shared (several people, one server)

People must trust the server's owner: root can read every account. Separation between people is by Linux account.

1. Install `bombd` to `/usr/local/bin`, create the gateway's user: `useradd --system --user-group --shell /usr/sbin/nologin bombd`.
2. Install `bombd-gateway.service`, `bombd-core@.socket`, `bombd-core@.service` into `/etc/systemd/system`,
   `bombd.tmpfiles.conf` into `/etc/tmpfiles.d/bombd.conf` (then `systemd-tmpfiles --create`),
   and `bombd.sudoers` into `/etc/sudoers.d/bombd` (mode 0440; check with `visudo -c`).
3. Put `BOMBD_PUBLIC=<your-server-address>:7443` in `/etc/bombd/gateway.env`.
4. `systemctl enable --now bombd-gateway`.
5. Make yourself the admin. Your own account keeps its login name:
   `sudo -u bombd bombd add-user --data /var/lib/bombd --name <you> --socket /run/bombd/<you>.sock --admin`,
   `systemctl enable --now bombd-core@<you>.socket`, then
   `sudo -u bombd bombd invite --data /var/lib/bombd --public <address>:7443 --user <you>` and pair your Mac.
6. Invite others from Bomb Code → Settings → Servers → "Invite a person". Each gets an account named `bc-…`,
   a private home, their own core and their own AI sign-ins.

The agent CLIs (`claude`, `codex`, `grok`), `git` and Node must be installed system-wide so every account can run them.
