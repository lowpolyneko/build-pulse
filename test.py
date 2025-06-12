#!/usr/bin/env python
import sqlite3

con = sqlite3.connect("data.db")
cur = con.cursor()

res = cur.execute("SELECT snippet_start, snippet_end, log_id FROM issues")
start, end, log_id = res.fetchone()

print((start,end,log_id))
res = cur.execute("SELECT data FROM logs WHERE id = ?", (log_id,))
data, = res.fetchone()

print(data.encode()[start:end].decode())
