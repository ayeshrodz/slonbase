# Automations

An automation is a list of steps that runs on a schedule or [when a record
changes](#when-a-record-changes). Each step can use what the steps before it
produced, so "every morning, find the invoices that are overdue and send each customer the
reminder email" needs no code:

1. **Find records** in `invoices` where `overdue = true && reminded = false`
2. **Continue only if** there are any
3. **Send a template** once for each record, to the record's `email`
4. **Change a record** once for each record: set `reminded` to true

You make them in the dashboard under **Automations**, or through `/api/automations`
([API](api.md)). The jobs of hook files (`cronAdd`) and the automatic backup are listed there too,
with the history of everything that ran.

## Steps

| Step | What it does | What it gives the steps after it |
|---|---|---|
| Find records (`query`) | Reads up to 1000 records of a collection, with a filter and a sort | `rows`, `count` |
| Continue only if (`condition`) | Ends the run, successfully, unless an expression holds | `passed` |
| Send a template (`email`) | Sends an [email template](#email-templates) | `sent` |
| Change a record (`write`) | Creates, updates or deletes a record | `id` |
| Call a URL (`http`) | An HTTP request to a public address | `status`, `json`, `text` |
| Send a signed webhook (`webhook`) | A JSON `POST` its receiver can verify ([below](#signed-webhooks)) | `status` |
| Run a hook function (`script`) | A function a hook file registered ([below](#functions-of-hook-files)) | `result` |
| Send a plain email (`mail`) | A plain-text email | `sent` |
| Clean up records (`cleanup`) | Deletes the records that match a filter or are older than some days | `deleted` |
| Run SQL (`sql`) | A statement, under the SQL console's policy (owners only) | `rows`, `count`, `affectedRows` |

An automation has 1 to 20 steps and a run has five minutes per attempt.

## Passing data on

Every step has a name. A step called `overdue` is `steps.overdue` to the steps after it, in
[Jinja](https://jinja.palletsprojects.com/en/stable/templates/) expressions:

- `{{ steps.overdue.count }}` in a text: an email's subject, a URL, a request body, a field to set.
- `steps.overdue.count > 0` as the expression of a condition.
- `steps.overdue.rows` as the list of **Once for each item of**: the step then runs once per
  item (up to 1000), and the item is `item` in that step: `{{ item.email }}`, `{{ item.id }}`.

Also there: `index` (the item's position, from 0), `automation.name`, `automation.id`, `now`
and `trigger` (what started the run: `trigger.type` is `schedule`, `manual` or `record`).
A value that isn't there is nothing, not an error. Filters work as in Jinja (`| upper`,
`| default("…")`, `| length`, `| int`, `| tojson`), and `| date("%d %B %Y")` writes a date.
Comparisons, `and`, `or`, `not`, `-` and `/` work; `+`, `*`, `**` and `~`, and `{% set %}`,
`{% macro %}` and `{% filter %}` blocks don't: they can make a text far larger than what the
template was given. Compute such values in a step (a query or SQL) instead.

Three things are never built from data, so data can't change what they do:

- **A filter** takes data through its parameters: write `status = {:status}` and give
  `{ "status": "{{ item.status }}" }` as the parameters. The value is bound, not pasted in.
- **An SQL statement** is run exactly as it was saved. What it returns reaches later steps as
  text (`{{ steps.totals.rows[0].sum | int }}`).

- **The server a URL step calls** is typed: data can fill in the path and the query
  (`https://example.com/orders/{{ item.id }}`), not the scheme or the host, because the request
  carries the step's saved headers. A header that carries a credential (`Authorization`,
  `Cookie`, `X-Api-Key`, ...) is only sent to an `https://` address: with `http://` the step
  isn't saved.

Records reach steps without the fields no API shows (password hashes, token keys), and system
collections can't be read or written by a step.

## When a record changes

Instead of a schedule, an automation can run when a record of a collection is created, updated
or deleted: **Runs → When a record changes** in the editor (`trigger` in the API). The steps see
the change as `trigger`:

| | |
|---|---|
| `trigger.action` | `create`, `update` or `delete` |
| `trigger.record` | The record after the change (before it, for a delete), without the fields no API shows |
| `trigger.changed` | For an update, the names of the fields whose value changed |

**Only if** is an expression over `trigger`: the automation only starts for a change it holds
for, e.g. `trigger.record.status == "paid" and "status" in trigger.changed`. A change it doesn't
hold for leaves no run in the history.

How it behaves:

- **Every change counts, whatever made it:** the API, a hook, the SQL console, another program
  writing to the table. A change that was rolled back starts nothing.
- **Once per change.** Each change starts each automation that watches for it once, also with
  several instances and across a restart: the runs are queued in the same step that notes how
  far changes were read. A run is retried like any other.
- **It ends.** A change an automation makes carries the automations it came through: none is
  started twice in such a chain, so an automation that updates the record that started it
  doesn't start itself again, and a chain is at most three automations long. This covers what a
  run writes through its steps, SQL and hook functions included, not what a server it calls
  writes back through the API.
- **At most 1000 runs of one automation wait at once.** Changes beyond that are not run, and the
  history has an entry that says how many. More than 5000 changes at once (a bulk change) are
  not run past the first 5000.
- **Not later.** A change made before the automation was made or switched on doesn't start
  it, however late the change is read. Neither does one made while automations were paused. A
  change counts from when its transaction began. A restore or an import writes records without starting
  anything.
- Runs of the same automation for different changes can overlap (there are two workers), so
  don't rely on their order.

A change waits in the database until it is turned into runs, for up to a day, so nothing is lost
while no instance is up. One that is older than that when it is read (from a transaction left
open for a day) is not run. Health warns when changes have been waiting for more than a minute.

## Signed webhooks

A **Send a signed webhook** step sends a JSON `POST` to an `https://` address, signed the
[Standard Webhooks](https://www.standardwebhooks.com/) way, so the receiver can check that it
came from here and wasn't changed, with a library that exists for most languages:

| Header | |
|---|---|
| `webhook-id` | The delivery's id: the same on every retry, so a receiver can drop one it already handled |
| `webhook-timestamp` | When this attempt was sent (seconds since 1970); refuse old ones |
| `webhook-signature` | `v1,` and the base64 of HMAC-SHA256 over `{id}.{timestamp}.{body}` |

- **What is sent** is the JSON you give, with values from the run (`{ "order": "{{ trigger.record.id }}" }`).
  Empty, it is `{ automation, at, trigger, item }`.
- **The secret** (`whsec_…`) is made by the server when the step is saved and **shown once**:
  give it to the receiver. Afterwards it is hidden. **Replace it** makes a new one; the old one
  keeps signing next to it for 24 hours (the header then has two signatures), so the receiver
  can switch without missing a delivery.
- A status other than 2xx fails the step, and it is retried with the automation's retries. A
  redirect is not followed: it counts as a failure, so what is sent only goes to the address
  you gave.
- The webhook that tells you about failures is signed the same way, with a secret of its own,
  when its address is `https://` (from the automation's next save, for one saved before 2.7).
  An `http://` one is sent unsigned, as before.

Verifying in Node.js:

```js
import { Webhook } from "standardwebhooks";
const payload = new Webhook(process.env.WEBHOOK_SECRET).verify(rawBody, request.headers);
```

## Functions of hook files

What steps can't express is a function in a [hook file](extending.md):

```js
automationStep("score", (input, context) => {
  const order = $app.findRecordById("orders", input.id);
  return { score: order.getInt("total") > 1000 ? "high" : "normal" };
});
```

A **Run a hook function** step chooses the function and gives it its input (JSON, with values
from the run). What it returns is `steps.<name>.result`. `context` has `automation`, `now`,
`item`, `index` and `trigger`. A function has five minutes; an error it throws fails the step
(an `ApiError` with a 4xx status is not retried). Admins can use any registered function; what
a function may do is decided by whoever deploys the hook files.

## Email templates

A template is an email written once: a subject and an HTML body in Jinja syntax, under
**Automations → Templates**. The editor shows the email as it will look, with sample data you
give it, and can send you a test.

- Values are written with `{{ item.name }}` and **escaped**: data can't bring markup into the
  email. `{% if %}` and `{% for %}` work as in Jinja; there is no include or import.
- The subject must come to one line.
- The body is put in the mail layout of Settings → Mail, like every other email.
- A "Send a template" step gives the template `item`, `steps` and whatever you put in its
  **extra data**.
- A template an automation sends can't be deleted.
- Code sends one with `$templates.send(id, to, data)` in a hook file, and other systems with
  `POST /api/templates/{id}/send` ([API](api.md)): `{ "to": …, "data": { … } }`, for developers,
  admins and owners (an API key works). The request is checked at once (the template, the
  addresses, that it renders) and answered `202`; the emails are sent in the background, each
  address tried up to three times. An `Idempotency-Key` header makes a request safe to repeat
  for seven days (the key is the caller's own; used again with other addresses or data, it is
  refused with `422`). One caller can queue 100 emails a minute.

A schedule that sends email (either kind of step) is refused when its next 24 runs, worked out
as it is saved, include two less than five minutes apart. This is a check of the schedule, not a
sending limit: an unusual expression can still bring two runs closer later on, and it doesn't
apply to runs started by hand or by a record change, nor to how many emails a step sends once
for each item. Each step run sends to at most ten addresses.

## When a step fails

The run stops at the step and is recorded as failed, with the step's name in the message. With
**If it fails, try again**, the run is tried again later (after the wait you choose, then twice
as long each time), and it **continues at the step that failed**: steps that finished are not
done again, a step that runs once per item goes on with the next item, and an email to several
addresses goes on with the next address. So an email that was sent is not sent a second time
(unless the server stops, or the database can't be written, in the instant between sending one
and noting it: then that one email can be sent again). Retries survive a restart. An automation that is changed while
a retry waits is not continued: that run is recorded as failed.

A failure that trying again can't fix (a missing template, an address that isn't one, a record
that isn't there) is not retried.

You can be told when an automation starts failing and when it works again, by email or by a
webhook.

## Who may do what

| | Viewer | Developer | Admin | Owner |
|---|---|---|---|---|
| See that an automation exists, its schedule and how it ran | ✓ | ✓ | ✓ | ✓ |
| Run one now | | ✓ | ✓ | ✓ |
| See its steps, make, change and delete | | | ✓ | ✓ |
| The same for one with an SQL step, and run it now | | | | ✓ |
| Read templates | ✓ | ✓ | ✓ | ✓ |
| Preview a template | | ✓ | ✓ | ✓ |
| Send a template through the API | | ✓ | ✓ | ✓ |
| Make, change, delete and test-send templates | | | ✓ | ✓ |

The history shows what each step came to in numbers (how many records, which status, how many
emails), never what was read or answered.

## Through the API

`/api/automations` and `/api/crons` serve the same jobs, with the same roles and fields
([API](api.md)). `GET` lists every job with its schedule in words, its next run and its last
run; `POST /api/automations/{id}` (or `/api/crons/{id}`) runs one now. A job's settings
(`config`, `notify`) are returned to admins and owners only, and an SQL step's statement to
owners only.

A job is `{name, expression, type, enabled, config, retries?, retryDelaySeconds?, notify?, copyOf?}`:

- **`name`** is one line of plain text. Ids of the jobs made this way start with `user:`.
- **`expression`** is the cron schedule. Instead of it, **`trigger`**
  `{type: "record", collection, actions: ["create", "update", "delete"], condition?}` starts the
  job when a record changes; its steps see `trigger.action`, `trigger.record` and
  `trigger.changed`.
- **`type`** is `steps` for an automation, whose `config` is
  `{steps: [{name, type, config, forEach?}]}` (1 to 20 steps). A job of one of the older single
  types (`cleanup`, `http`, `mail`, `sql`) has that step's `config` as its own.
- **`retries`** (0 to 5) tries again after a failed run, waiting `retryDelaySeconds` first (1 to
  300), then twice as long each time.
- **`notify`** is `{emails, webhook}`: told once when a scheduled run starts failing and once when
  it works again. The webhook is a JSON `POST` to a public URL, shown as `••••••••`. An https one
  is signed (an http one is sent unsigned): its secret is returned once, in the answer's
  `secrets.notify`, and `notify.rotateSecret: true` replaces it. An https webhook saved before
  2.7 has no secret and is sent unsigned until the job is saved again, which gives it one.
- **`copyOf`**, on `POST`, is the id of a job to copy the saved secrets from; header values only
  follow when the URL is unchanged.

The `config` of each step type:

| `type` | `config` |
|---|---|
| `query` | `collection`, `filter`, `params`, `sort`, `limit` |
| `condition` | `expression` |
| `email` | `template`, `to`, `data` |
| `write` | `action` (`create`, `update` or `delete`), `collection`, `id`, `data` |
| `http` | `url`, `method`, `headers`, `body`, `timeoutSeconds`, `expectStatus`. Public addresses only; a credential header (`Authorization`, `Cookie`, `X-Api-Key`, ...) only with an `https://` URL. Header values come back as `••••••••`, and the mask keeps the saved value |
| `webhook` | `url` (https only), `payload`, `timeoutSeconds`. Its `secret` is made by the server and returned once, in the answer's `secrets.steps.<name>`, then shown as `••••••••`; `rotate: true` makes a new one, and the old one keeps signing for 24 hours |
| `script` | `function` (registered by a hook file with `automationStep`), `input` |
| `mail` | `to`, `subject`, `body`; needs SMTP on |
| `cleanup` | `collection`, `filter`, `olderThanDays`, `field`, `maxRows` |
| `sql` | `statement`; owners only, only with the SQL console on, run under the console's policy |

Texts in a step's `config` may use what earlier steps gave (`{{ steps.<name>.count }}`, and
`{{ item }}` in a step with `forEach`); a filter and an SQL statement may not. A job with an `sql`
step is an owner's, like a job of type `sql`.

`POST /api/cron-preview` checks a job without saving it: its schedule in words, the next runs
and, for a cleanup, how many records match now. `GET /api/jobs/runs?kind=cron&job=<id>` is its
history; a run's `meta` is `{steps: [{name, type, ...counts}], stoppedAt?}`.

## Cron jobs made before 2.7

A cron job made in Settings → Cron jobs is an automation of one step. It runs as it did, keeps
its id and its history, and `/api/crons` still works. Editing one in the dashboard saves it as
steps; its saved header values stay.
