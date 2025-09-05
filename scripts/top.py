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

from datetime import datetime
import logging
from os import environ
import sqlite3

from matplotlib.axes import Axes
from matplotlib.dates import DateFormatter
from matplotlib.ticker import PercentFormatter
import numpy
import matplotlib.pyplot as plt


def plot_run(
    tops: list[str],
    name: str,
    cpu_ax: Axes,
    mem_ax: Axes,
) -> None:
    ts: list[datetime] = []
    cpu_usage: list[float] = []
    mem_usage: list[float] = []

    # 21 includes trailing whitespace per report
    for top in [tops[i : i + 20] for i in range(0, len(tops), 21)]:
        logging.debug(
            f"--- {name} ---\n{'\n'.join([' '.join(line) for line in top])}\n--- {name} ---"
        )
        ts.append(datetime.strptime(top[0][2], "%H:%M:%S"))  # top - <time>
        cpu_usage.append(float(top[2][7]))  # %Cpu(s): id
        mem_usage.append(
            float(top[3][5]) / float(top[3][3]) * 100
        )  # MiB Mem : <xx> free / <xx> total

    _ = cpu_ax.plot(numpy.array(ts), cpu_usage, label=name, ls=":")
    _ = mem_ax.plot(numpy.array(ts), mem_usage, ls=":")


def main() -> None:
    logging.basicConfig(level=environ.get("LOGLEVEL", "WARNING").upper())
    fig, (ax1, ax2) = plt.subplots(2, 1, sharex=True, sharey=True)

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
            tops = [line.split() for line in contents.decode("utf-8").split("\n")]
            if all(
                match not in display_name for match in ["bsd", "aarch64", "xpmem"]
            ):  # BSD and AArch64 top output differs
                plot_run(tops, display_name, ax1, ax2)
                break  # TODO output is not meaningful when considering the entire set of agents

    ax2.set_xlabel("Timestamp")
    ax2.xaxis.set_major_formatter(DateFormatter("%H:%M:%S"))
    fig.autofmt_xdate()

    ax1.set_ylabel("CPU(s) Idle%")
    ax1.set_ybound(0, 100)
    ax1.yaxis.set_major_formatter(PercentFormatter())

    ax2.set_ylabel("Mem Free%")

    _ = fig.suptitle("Jenkins Agent System Usage (~1 min intervals)")
    _ = fig.legend(loc="lower right")
    fig.set_size_inches(fig.get_figwidth(), fig.get_figheight() * 2)
    fig.savefig("usage.png")


if __name__ == "__main__":
    main()
