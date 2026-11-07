#!/usr/bin/env python3
"""Writes docs/assets/topology-*.svg (auto light/dark) and -light/-dark variants for the site; run: python3 scripts/topology-diagrams.py"""
OUT = '/home/user/slonbase/docs/assets/'
STYLE = '''<style>
:root{--bg:#fff;--box:#fff;--line:#c4ccd4;--ink:#1f2328;--mute:#5d6670;--acc:#4f7a6c;--accbg:#e8f0ed;--pg:#5f7593;--pgbg:#e9eef5;--warn:#9a6b2f}
@media (prefers-color-scheme:dark){:root{--bg:#171a1d;--box:#20252a;--line:#3a424a;--ink:#e6e9ec;--mute:#9aa4ae;--acc:#7fb09f;--accbg:#233029;--pg:#8fa6c4;--pgbg:#232b36;--warn:#d1a05f}}
text{font:500 13px "IBM Plex Sans",system-ui,sans-serif;fill:var(--ink)}
.s{font-size:11.5px;fill:var(--mute);font-weight:400}
.t{font-weight:700;font-size:13px}
.box{fill:var(--box);stroke:var(--line);stroke-width:1.2}
.us{fill:var(--accbg);stroke:var(--acc);stroke-width:1.4}
.pg{fill:var(--pgbg);stroke:var(--pg);stroke-width:1.4}
.ln{stroke:var(--line);stroke-width:1.5;fill:none}
.ac{stroke:var(--acc);stroke-width:1.6;fill:none}
.pgl{stroke:var(--pg);stroke-width:1.6;fill:none}
.dash{stroke-dasharray:5 4}
.ar{fill:var(--mute)}
</style>'''

def box(x, y, w, h, title, sub='', cls='box'):
    s = f'<rect class="{cls}" x="{x}" y="{y}" width="{w}" height="{h}" rx="8"/>'
    if sub:
        s += f'<text class="t" x="{x+w/2}" y="{y+h/2-3}" text-anchor="middle">{title}</text><text class="s" x="{x+w/2}" y="{y+h/2+14}" text-anchor="middle">{sub}</text>'
    else:
        s += f'<text class="t" x="{x+w/2}" y="{y+h/2+5}" text-anchor="middle">{title}</text>'
    return s

def line(x1, y1, x2, y2, cls='ln', arrow=False):
    return f'<path class="{cls}" d="M{x1} {y1} L{x2} {y2}"/>'

def label(x, y, t, anchor='middle'):
    return f'<text class="s" x="{x}" y="{y}" text-anchor="{anchor}">{t}</text>'

import re
def svg(name, w, h, body, title):
    def write(fn, style):
        open(OUT + fn, 'w').write(
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-label="{title}">'
            f'<title>{title}</title>{style}<rect width="{w}" height="{h}" fill="var(--bg)" rx="10"/>{body}</svg>')
    write(name, STYLE)
    m = re.search(r'@media \(prefers-color-scheme:dark\)\{:root\{(.*?)\}\}', STYLE)
    light = STYLE.replace(m.group(0), '')
    dark = STYLE.replace(m.group(0), '').replace(':root{--bg:#fff;', ':root{' + m.group(1) + ';--bgx:0;', 1)
    dark = re.sub(r':root\{(--bg:#171a1d.*?);--bgx:0;(.*?)\}', lambda k: ':root{' + m.group(1) + '}', dark, count=1, flags=re.S) if False else STYLE.replace(m.group(0), '').replace(re.search(r':root\{.*?\}', STYLE).group(0), ':root{' + m.group(1) + '}', 1)
    write(name.replace('.svg', '-light.svg'), light)
    write(name.replace('.svg', '-dark.svg'), dark)

# 1. one instance
b = box(30, 110, 130, 60, 'Clients', 'SDKs, browsers, apps')
b += box(260, 95, 200, 90, 'slonbase', 'API · auth · files · realtime · dashboard', 'us')
b += box(560, 110, 170, 60, 'PostgreSQL', 'your data: ordinary tables', 'pg')
b += line(160, 140, 260, 140, 'ln') + line(460, 140, 560, 140, 'pgl')
b += label(210, 128, 'HTTPS') + label(510, 128, 'SQL')
b += label(30, 225, '≈ 3 connections idle, up to 18 under load (16 pooled + 2 listeners)', 'start')
svg('topology-single.svg', 760, 260, b, 'One slonbase instance on one PostgreSQL server')

# 2. several instances
b = box(20, 135, 110, 60, 'Clients')
b += box(170, 135, 120, 60, 'Load balancer', 'any, no affinity')
for i, y in enumerate((40, 135, 230)):
    b += box(350, y, 150, 60, f'slonbase {i+1}', '', 'us')
    b += line(290, 165, 350, y + 30, 'ln')
    b += line(500, y + 30, 590, 165, 'pgl')
b += line(130, 165, 170, 165)
b += box(590, 125, 150, 80, 'PostgreSQL', 'data · LISTEN/NOTIFY', 'pg')
b += box(350, 310, 150, 44, 'S3 or shared volume', '', 'box')
b += line(425, 290, 425, 310, 'ln dash')
b += label(20, 380, 'Files must be shared: S3, or one directory mounted on every instance.', 'start')
b += label(590, 225, 'shared through PostgreSQL:', 'start') + label(590, 241, 'tokens, schema, settings,', 'start') + label(590, 257, 'rate limits, realtime, cron', 'start')
svg('topology-fleet.svg', 760, 390, b, 'Several slonbase instances behind a load balancer on one PostgreSQL server')

# 3. with a pooler
b = box(10, 135, 100, 60, 'Clients')
b += box(140, 135, 110, 60, 'Load balancer')
for i, y in enumerate((30, 125, 220)):
    b += box(290, y, 130, 56, f'slonbase {i+1}', '', 'us')
    b += line(250, 165, 290, y + 28, 'ln')
    b += line(420, y + 28, 500, 165, 'pgl')
b += line(110, 165, 140, 165)
b += box(500, 135, 110, 60, 'PgBouncer', 'transaction mode', 'box')
b += box(650, 135, 100, 60, 'PostgreSQL', '', 'pg')
b += line(610, 165, 650, 165, 'pgl')
b += '<path class="ac dash" d="M355 286 L355 318 L700 318 L700 195"/>'
b += label(527, 312, 'LISTEN/NOTIFY: direct (database.listen_url)')
b += label(20, 350, 'Queries go through the pooler; only each instance\'s listeners connect directly.', 'start')
svg('topology-pooler.svg', 760, 365, b, 'Several slonbase instances behind PgBouncer, with listeners connected directly')

# 4. projects across servers
b = box(20, 125, 110, 60, 'Clients')
b += box(190, 105, 170, 100, 'slonbase', 'one or many instances', 'us')
b += line(130, 155, 190, 155)
b += box(470, 20, 260, 140, '', '', 'pg')
b += f'<text class="t" x="490" y="44">PostgreSQL server A</text><text class="s" x="490" y="64">main database: shop</text><text class="s" x="490" y="84">project database: shop_blog</text><text class="s" x="490" y="104">project database: shop_forum</text>'
b += box(470, 195, 260, 100, '', '', 'pg')
b += f'<text class="t" x="490" y="219">PostgreSQL server B</text><text class="s" x="490" y="239">project "eu-customers"</text><text class="s" x="490" y="259">connected with databaseUrl</text>'
b += line(360, 140, 470, 90, 'pgl') + line(360, 170, 470, 245, 'pgl')
b += label(20, 330, 'Each project is a separate installation: its own users, collections and settings.', 'start')
svg('topology-projects.svg', 760, 350, b, 'One slonbase serving a main database and projects on two PostgreSQL servers')
print('ok')
