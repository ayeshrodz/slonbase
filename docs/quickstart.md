# Quickstart: a self-hosted backend in five minutes

This takes a bare Linux server to a running backend with an API, sign-in, realtime and files, on
your own PostgreSQL. You need a Debian, Ubuntu, Fedora, RHEL, Rocky or Alma server (amd64 or
arm64) and `sudo`. For Docker, see [installation](installation.md#docker-compose).

## 1. Install

```sh
curl -fsSL https://slonbase.com/install.sh | sudo sh
```

It installs PostgreSQL if the machine has none, creates a role and a database for slonbase,
installs slonbase and creates your first superuser, asking for your email and printing a
generated password. See [installation](installation.md) for every step by hand.

The server listens on `127.0.0.1:8090`. From your computer, open a tunnel and then
<http://127.0.0.1:8090/_/>:

```sh
ssh -L 8090:127.0.0.1:8090 you@your-server
```

## 2. Create a collection

Sign in to the dashboard, choose **New collection**, name it `posts` and add the fields `title`
(text) and `published` (bool). Under **API rules**, make the list and view rules public (empty) for
this tryout.

## 3. Call the API

```sh
curl -s http://127.0.0.1:8090/api/health
curl -s http://127.0.0.1:8090/api/collections/posts/records
```

Creating records needs a token. Sign in as the superuser and send it:

```sh
TOKEN=$(curl -s -X POST http://127.0.0.1:8090/api/collections/_superusers/auth-with-password \
  -H 'content-type: application/json' \
  -d '{"identity":"you@example.com","password":"your password"}' | jq -r .token)

curl -s -X POST http://127.0.0.1:8090/api/collections/posts/records \
  -H "authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"title":"Hello, slonbase","published":true}'
```

## 4. Use an SDK and realtime

The PocketBase JavaScript SDK works against slonbase's API:

```js
import PocketBase from "pocketbase";

const pb = new PocketBase("http://127.0.0.1:8090");

// realtime: this runs for every change to posts, from anywhere
pb.collection("posts").subscribe("*", (e) => console.log(e.action, e.record.title));

await pb.collection("posts").create({ title: "Live", published: true });
const page = await pb.collection("posts").getList(1, 20, { filter: "published = true" });
```

Because every collection is a table, the same data is one SQL query away:

```sh
sudo -u postgres psql slonbase -c 'select id, title from slonbase.posts'
```

## Next

- Put a reverse proxy with HTTPS in front: [deployment](deployment.md#reverse-proxy).
- Add users with an auth collection, and rules to match: the [API guide](api.md).
- Run more than one app from the same server: [projects](projects.md).
- Extend it with JavaScript [hooks](extending.md).
