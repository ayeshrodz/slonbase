# Automations

An automation is a schedule and a list of steps. Each step can use what the steps before it
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

Also there: `index` (the item's position, from 0), `automation.name`, `automation.id` and `now`.
A value that isn't there is nothing, not an error. Filters work as in Jinja (`| upper`,
`| default("…")`, `| length`, `| int`, `| tojson`), and `| date("%d %B %Y")` writes a date.

Two things are never built from data, so data can't change what they do:

- **A filter** takes data through its parameters: write `status = {:status}` and give
  `{ "status": "{{ item.status }}" }` as the parameters. The value is bound, not pasted in.
- **An SQL statement** is run exactly as it was saved. What it returns reaches later steps as
  text (`{{ steps.totals.rows[0].sum | int }}`).

Records reach steps without the fields no API shows (password hashes, token keys), and system
collections can't be read or written by a step.

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

What sends email (either kind of step) can run at most every five minutes, and to at most ten
addresses per step run.

## When a step fails

The run stops at the step and is recorded as failed, with the step's name in the message. With
**If it fails, try again**, the run is tried again later (after the wait you choose, then twice
as long each time), and it **continues at the step that failed**: steps that finished are not
done again, and a step that runs once per item goes on with the next item. So an email that was
sent is not sent a second time. Retries survive a restart.

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
| Make, change, delete and test-send templates | | | ✓ | ✓ |

The history shows what each step came to in numbers (how many records, which status, how many
emails), never what was read or answered.

## Cron jobs made before 2.7

A cron job made in Settings → Cron jobs is an automation of one step. It runs as it did, keeps
its id and its history, and `/api/crons` still works. Editing one in the dashboard saves it as
steps; its saved header values stay.
