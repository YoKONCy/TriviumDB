"""按共享独立样例核对实际 Python API 的错误类别与完整字段。"""

import json
from pathlib import Path
import triviumdb


def run_error_contract(root):
    contract = json.loads((Path(__file__).parents[1] / "contracts/error_cases.json").read_text(encoding="utf-8"))
    for case in contract["cases"]:
        operation = case["operation"]
        directory = Path(root) / f"errors-{operation}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        try:
            db.insert_with_id(1, [1, 0], {"kind": "alpha"})
            db.insert_with_id(2, [0, 1], {"kind": "beta"})
            if operation in ("readonly", "rows"):
                db.close()
                db = triviumdb.TriviumDB(file, dim=2, access_mode="read_only", max_query_rows=1)
            if operation == "unique":
                db.create_unique_index("kind")
            if operation == "closed":
                db.close()
            if operation == "io":
                (directory / "blocked").write_text("文件占据父路径", encoding="utf-8")
            if operation == "version":
                header = bytearray(58)
                header[:4] = b"TVDB"
                header[4:6] = (65535).to_bytes(2, "little")
                (directory / "future.tdb").write_bytes(header)
            actions = {
                "missing_parameter": lambda: db.execute_prepared_tql(db.prepare_tql("MATCH (n) RETURN $required AS value"), {}),
                "unexpected_parameter": lambda: db.execute_prepared_tql(db.prepare_tql("MATCH (n) RETURN $required AS value"), {"required": 1, "extra": 2}),
                "dimension": lambda: db.insert([1], {}),
                "missing_node": lambda: db.update_payload(999, {}),
                "duplicate_node": lambda: db.insert_with_id(1, [1, 0], {}),
                "unique": lambda: db.insert_with_id(3, [1, 0], {"kind": "alpha"}),
                "readonly": lambda: db.insert([1, 0], {}),
                "rows": lambda: db.tql("MATCH (n) RETURN n"),
                "migration": lambda: db.tql_mut("MATCH (n) RETURN n"),
                "closed": lambda: db.get_payload(1),
                "locked": lambda: triviumdb.TriviumDB(file, dim=2),
                "io": lambda: triviumdb.TriviumDB(str(directory / "blocked/database.tdb"), dim=2),
                "version": lambda: triviumdb.TriviumDB(str(directory / "future.tdb"), dim=2),
            }
            try:
                actions[operation]()
            except triviumdb.TriviumDBError as error:
                assert error.code == case["code"], (operation, error.code)
                details = dict(error.details)
                for field in case.get("string_fields", []):
                    value = details.pop(field)
                    assert isinstance(value, str) and value, (operation, field)
                for field in case.get("integer_fields", []):
                    assert type(details.pop(field)) is int, (operation, field)
                assert details == case["details"], (operation, details)
                for field, value in error.details.items():
                    assert getattr(error, field) == value, (operation, field)
            else:
                raise AssertionError(f"操作应返回结构化错误：{operation}")
        finally:
            db.close()
