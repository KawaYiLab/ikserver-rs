"""Validate exported story MCP schemas and recorded responses (requires jsonschema)."""
import json
import sys
from pathlib import Path

from jsonschema import Draft202012Validator


def validate(schema_report: Path, workflow_report: Path) -> None:
    report = json.loads(schema_report.read_text(encoding="utf-8"))
    schemas = {tool["name"]: tool["outputSchema"] for tool in report["definitions"]["tools"]}
    for schema in schemas.values():
        Draft202012Validator.check_schema(schema)
    count = 0
    for recorded in report["responses"]:
        Draft202012Validator(schemas[recorded["tool"]]).validate(
            recorded["response"]["result"]["structuredContent"]
        )
        count += 1
    workflows = json.loads(workflow_report.read_text(encoding="utf-8"))
    for query in workflows["queries"]:
        Draft202012Validator(schemas[query["tool"]]).validate(
            query["output"]["result"]["structuredContent"]
        )
        count += 1
    print(f"Validated {len(schemas)} schemas and {count} recorded responses")


if __name__ == "__main__":
    validate(Path(sys.argv[1]), Path(sys.argv[2]))
