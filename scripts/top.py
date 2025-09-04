#!/usr/bin/env -S uv run --script
#
# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "matplotlib",
# ]
# ///
"""
Graphs all jobs by their `top.txt` resource usage.
"""

# pyright: reportAny=false
# pyright: reportUnknownMemberType=false

import matplotlib.pyplot as plt
import sqlite3


def main() -> None:
    with sqlite3.connect("data.db") as con:
        cur = con.cursor()
        res = cur.execute(
            " \
            SELECT display_name,contents FROM artifacts \
            JOIN runs ON artifacts.run_id = runs.id \
            WHERE path = 'top.txt' \
            "
        )

        for display_name, contents in res.fetchall():
            print(
                f"--- {display_name} ---\n{contents.decode('utf-8')}\n--- {display_name} ---"
            )

    # _ = plt.plot([1, 2, 3, 4])
    # plt.savefig("usage.png")


if __name__ == "__main__":
    main()
