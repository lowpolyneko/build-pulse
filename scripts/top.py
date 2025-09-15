#!/usr/bin/env -S uv run --script
#
# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "matplotlib",
# ]
# ///
"""
Graphs a job by its `top.txt` resource usage.
"""

# pyright: reportAny=false
# pyright: reportUnknownMemberType=false

from datetime import datetime
from os import environ
import argparse
import logging
import sqlite3
from sys import stdin, stdout

from matplotlib.axes import Axes
from matplotlib.dates import DateFormatter
from matplotlib.ticker import PercentFormatter
import numpy
import matplotlib.pyplot as plt


def plot_run(
    tops: list[list[str]],
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
        cpu_usage.append(100 - float(top[2][7]))  # %Cpu(s): id
        mem_usage.append(
            100 - float(top[3][5]) / float(top[3][3]) * 100
        )  # MiB Mem : <xx> free / <xx> total

    _ = cpu_ax.plot(numpy.array(ts), cpu_usage, label=name)
    _ = mem_ax.plot(numpy.array(ts), mem_usage)


def main() -> None:
    logging.basicConfig(level=environ.get("LOGLEVEL", "WARNING").upper())
    parser = argparse.ArgumentParser()
    _ = parser.add_argument("-u", "--url")
    _ = parser.add_argument(
        "-d", "--db", nargs="?", const=1, type=str, default="data.db"
    )
    _ = parser.add_argument("-o", "--output")
    args = parser.parse_args()

    fig, (ax1, ax2) = plt.subplots(2, 1, sharex=True, sharey=True)

    if args.url is not None:
        with sqlite3.connect(args.db) as con:
            cur = con.cursor()
            res = cur.execute(
                " \
                SELECT display_name,contents FROM artifacts \
                JOIN runs ON artifacts.run_id = runs.id \
                WHERE path = 'top.txt' AND url = ? \
                ",
                [args.url],
            )

            display_name, contents = res.fetchone()
            contents = contents.decode("utf-8")
    else:
        display_name = environ.get("BUILD_PULSE_RUN_NAME", "unknown run")
        contents = stdin.read()

    if any(
        match in display_name for match in ["bsd", "aarch64", "xpmem"]
    ):  # BSD and AArch64 top output differs, just spit back the original for now
        print(contents)
        return

    tops = [line.split() for line in contents.split("\n")]
    plot_run(tops, display_name, ax1, ax2)

    ax2.set_xlabel("Timestamp")
    ax2.xaxis.set_major_formatter(DateFormatter("%H:%M:%S"))
    fig.autofmt_xdate()

    ax1.set_ylabel("CPU Usage%")
    ax1.set_ybound(0, 100)
    ax1.yaxis.set_major_formatter(PercentFormatter())

    ax2.set_ylabel("Mem Usage%")

    _ = fig.suptitle("Jenkins Agent System Usage (~1 min intervals)")
    _ = fig.legend(loc="lower right")
    fig.set_size_inches(fig.get_figwidth(), fig.get_figheight() * 2)
    if args.output:
        fig.savefig(args.output)
    else:
        fig.savefig(stdout.buffer)


if __name__ == "__main__":
    main()
