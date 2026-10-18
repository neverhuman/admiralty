#!/usr/bin/env python3
"""Write snapshot.html: index.html with the current status.json inlined (no polling)."""
import os, json
B = os.environ.get('CDASH_RUNTIME', '/home/ubuntu/xbabe2_kicker/captains-dashboard')
html = open(os.path.join(B, 'index.html')).read()
data = open(os.path.join(B, 'status.json')).read()
data = data.replace('</', '<\\/')
html = html.replace('/*INLINE_DATA*/null', data)
open(os.path.join(B, 'snapshot.html'), 'w').write(html)
print('snapshot.html written')
