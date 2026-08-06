import sqlite3

DB_PATH = "logs/anchor_log.sqlite"

conn = sqlite3.connect(DB_PATH)
cur = conn.cursor()

cur.execute(
    "SELECT id FROM anchor_log WHERE tool_name = 'Read' ORDER BY id LIMIT 1"
)
row = cur.fetchone()

if row is None:
    print("No 'Read' entries found.")
else:
    row_id = row[0]
    cur.execute(
        "UPDATE anchor_log SET tool_name = 'Write' WHERE id = ?",
        (row_id,),
    )
    conn.commit()
    print(f"Row {row_id} updated from Read to Write.")

conn.close()
