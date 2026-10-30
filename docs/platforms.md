# Deploying to a platform

Ready-made setups for hosting platforms, each with slonbase and a PostgreSQL database. They all
use the [image](installation.md#docker) from `ghcr.io/ayeshrodz/slonbase`.

- [Render](#render)
- [Railway](#railway)
- [Fly.io](#flyio)
- [Coolify](#coolify)
- [Dokploy](#dokploy)
- [After the first deploy](#after-the-first-deploy)

The templates use the `latest` image. To control upgrades, pin a release (`:1.1.0`) instead.

**Disks owned by root.** Render, Railway and Fly.io mount their disks at `/data` owned by root.
The image runs as an unprivileged user, which can start but can't write files there, so file
uploads would fail. The setups below run the container as root on those platforms
(`deploy/render/Dockerfile`, `deploy/fly/Dockerfile`, `RAILWAY_RUN_UID=0`). Coolify and Dokploy
use Docker volumes, which keep the image's ownership, so the image runs unchanged there.

## Render

<a href="https://render.com/deploy?repo=https://github.com/ayeshrodz/slonbase"><img alt="Deploy to Render" src="https://render.com/images/deploy-to-render-button.svg"></a>

The button reads [`render.yaml`](../render.yaml). It creates:

- a web service on the Starter plan, with a 1 GB disk at `/data` and `/api/health` as its health
  check;
- a PostgreSQL 17 database that only the service can reach, with its connection string passed to
  the service as `SLONBASE_DATABASE_URL`.

Disks need a paid instance. To use the free plan, remove the `disk` section and keep files on S3
(Settings → Files storage). Without a disk, hooks and local backups don't survive a redeploy.

Once it's live, find the setup link in the service's **Logs**. It shows `http://127.0.0.1:8090`;
replace that with the service's `https://….onrender.com` address.

## Railway

1. Create a project and add **PostgreSQL** (+ New → Database → PostgreSQL).
2. Add the app: + New → Docker Image → `ghcr.io/ayeshrodz/slonbase:latest`.
3. In the app's **Variables**, add:

   | Variable | Value |
   |---|---|
   | `SLONBASE_DATABASE_URL` | `${{Postgres.DATABASE_URL}}` |
   | `PORT` | `8090` |
   | `RAILWAY_RUN_UID` | `0` (see [disks owned by root](#deploying-to-a-platform)) |

4. Attach a volume to the app (right-click it → Attach volume), mounted at `/data`.
5. In Settings → Networking, generate a domain for port `8090`.

Open the setup link from the app's **Deploy Logs**, replacing `http://127.0.0.1:8090` with the
generated domain.

## Fly.io

[`deploy/fly/`](../deploy/fly) has a `fly.toml` and a two-line `Dockerfile` that runs the
image as root. The `fly.toml` sets up one machine with a 1 GB volume at `/data` and a health
check on `/api/health`.

```sh
mkdir slonbase-fly && cd slonbase-fly
curl -fsSLO https://raw.githubusercontent.com/ayeshrodz/slonbase/main/deploy/fly/fly.toml
curl -fsSLO https://raw.githubusercontent.com/ayeshrodz/slonbase/main/deploy/fly/Dockerfile
fly launch --copy-config --no-deploy         # pick an app name and a region
```

For the database, use [Fly Managed Postgres](https://fly.io/docs/mpg/) (`fly mpg create`) or any
PostgreSQL 14+ server. Then pass the database's connection string and a setup token, and deploy:

```sh
fly secrets set SLONBASE_DATABASE_URL='postgres://…' SLONBASE_SETUP_TOKEN="$(openssl rand -hex 24)"
fly deploy
```

Finish at `https://<app>.fly.dev/_/setup?token=<the token>`.

Keep it to one machine: the volume belongs to a single machine. To run several instances, keep
files on S3 and drop the volume (see [several instances](deployment.md#several-instances)).

## Coolify

Use [`deploy/coolify/slonbase.yaml`](../deploy/coolify/slonbase.yaml): + New → Resource →
Docker Compose Empty, then paste the file. Coolify generates the database credentials, a 64
character setup token and a URL. Deploy, then open `https://<the URL>/_/setup?token=<value of
SERVICE_PASSWORD_64_SETUP>` (see the resource's Environment Variables).

The file follows Coolify's one-click service template format, so it can be added to Coolify's
catalog (see [deploy/README.md](../deploy/README.md) for submitting it).

## Dokploy

[`deploy/dokploy/`](../deploy/dokploy) has a `docker-compose.yml` and a `template.toml` in the
format of Dokploy's template catalog. Without the catalog:

1. Create a service → Compose, and paste `docker-compose.yml`.
2. Under Environment, set `DB_PASSWORD` and `SETUP_TOKEN`. Use letters and digits, and make the
   token at least 32 characters (`openssl rand -hex 24` gives both).
3. Under Domains, add your domain, with service `slonbase` and port `8090`.
4. Deploy, then open `https://<domain>/_/setup?token=<SETUP_TOKEN>`.

## After the first deploy

In the dashboard's Settings:

- **Application URL:** set it to the public `https://` address. Emails link to it.
- **Trusted proxy:** every platform here puts a proxy in front of slonbase. Without this setting,
  logs, rate limits and `superuserIPs` see the proxy's address instead of the client's. Use the
  header your platform documents for the client's address: `Fly-Client-IP` on Fly.io, and
  usually `X-Forwarded-For` elsewhere (see [trusted proxy](configuration.md#trusted-proxy-and-superuser-ips)).
- **Mail:** configure SMTP so verification and password reset emails go out.

Realtime works through all of these platforms' proxies. For backups, the database is the
platform's PostgreSQL, so its own backups cover your records. slonbase's scheduled backups
(Settings → Backups) also include the files; send them to S3 when the disk isn't backed up.
