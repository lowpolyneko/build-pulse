#!/usr/bin/env -S uv run --script
#
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""
Parses outputted JUnit XML to report failing test cases per job.
"""

from sys import stdin
import xml.etree.ElementTree as ET


def main() -> None:
    testsuites = ET.fromstring(stdin.read())

    for testsuite in testsuites:
        failures = testsuite.attrib["failures"]
        errors = testsuite.attrib["errors"]
        skipped = testsuite.attrib["skipped"]
        total = testsuite.attrib["tests"]
        print(
            f"""Test Results

Failures: {failures}
Errors: {errors}
Skipped: {skipped}
Total: {total}

-----
"""
        )

        for testcase in testsuite:
            if any(testcase.tag == tag for tag in ["system-out", "system-err"]):
                continue  # not cases

            for child in testcase:
                if child.tag == "failure":
                    print(child.text)


if __name__ == "__main__":
    main()
