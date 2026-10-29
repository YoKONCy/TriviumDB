"""三端共享的 ID、边绑定、容器表达式与精确向量边界。"""
import json
import shutil
from pathlib import Path
import triviumdb


def run_tql_boundaries(root: str) -> None:
    contract = json.loads((Path(__file__).parents[1] / 'contracts/tql_boundary_cases.json').read_text(encoding='utf-8'))
    def open_db(name, **options):
        return triviumdb.TriviumDB(str(Path(root) / f'boundaries-{name}.tdb'), dim=1, auto_build_quiver=False, **options)
    def rows(db, query):
        return [row.row for row in db.tql(query)]
    def rejects(fn):
        try:
            fn()
        except (ValueError, RuntimeError):
            pass
        else:
            raise AssertionError('非法输入必须明确拒绝')
    aliases = json.loads((Path(__file__).parents[1] / 'contracts/tql_alias_cases.json').read_text(encoding='utf-8'))
    db = open_db('aliases')
    try:
        for record in aliases['records']:
            db.insert_with_id(record['id'], [1], record['payload'])
        for a, b in aliases['edges']:
            db.link(a, b, 'rel', 1)
        for indexed in [False, True]:
            if indexed:
                db.create_index('x')
            for case in aliases['cases']:
                result = rows(db, case['query'])
                if 'ids' in case:
                    assert [row['id'] for row in result] == case['ids'], case['query']
                else:
                    assert result == case['rows'], case['query']
        for query in aliases['invalid']:
            rejects(lambda: db.tql(query))
    finally:
        db.close()
    db = open_db('values')
    try:
        for row in contract['records']:
            db.insert_with_id(row['id'], [1], row['payload'])
        db.upsert_edge(1, 2, 'rel', 2, contract['edge_metadata'])
        for source in contract['sources']:
            for case in contract['cases']:
                query = source['query'] + ' ' + case['suffix'].replace('{var}', source['variable'])
                assert rows(db, query) == case['rows'], query
        for indexed in [False, True]:
            if indexed:
                db.create_index('x')
            for case in contract['edge_cases']:
                assert rows(db, case['query']) == case['rows'], case['query']
        assert rows(db, 'MATCH (a)-[e:rel]->(b) RETURN COALESCE(e,NULL) AS edge') == [{'edge': {'type': 'edge', 'source': 1, 'target': 2, 'label': 'rel', 'weight': 2, 'metadata': contract['edge_metadata']}}]
        for query in ['MATCH (n) RETURN 1 AS x, 2 AS x', 'MATCH (n) RETURN 1, 1', 'MATCH (n) RETURN $unbound', 'MATCH (n)-[n]->(b) RETURN n']:
            rejects(lambda: db.tql(query))
        prepared = db.prepare_tql('MATCH (n {id:1}) RETURN $a, $b')
        assert [row.row for row in db.execute_prepared_tql(prepared, {'a': 1, 'b': 1})] == [{'$a': 1, '$b': 1}]
    finally:
        db.close()
    db = open_db('id-index')
    try:
        for ident in [1, 2, 3]:
            db.insert_with_id(ident, [1], {'kind': 'sample', 'id': 4-ident})
        db.create_index('id'); db.create_ordered_index('id')
        assert rows(db, 'FIND {kind:"sample"} RETURN _.id AS id ORDER BY _.id ASC LIMIT 1') == [{'id': 1}]
        assert rows(db, 'SEARCH VECTOR [1] TOP 3 AS n WITH n WHERE n.id == 1 RETURN n.id AS id') == [{'id': 1}]
    finally:
        db.close()
    db = open_db('numeric-index')
    try:
        # 80 个候选覆盖索引选择路径，2 号节点持有数值相等的浮点表示。
        for ident in range(1, 81):
            db.insert_with_id(ident, [1], {'x': 1.0 if ident == 2 else ident})
        for indexed in [False, True]:
            if indexed:
                db.create_index('x')
            for literal in ['1', '1.0']:
                assert rows(db, f'MATCH (n) AS n WITH n WHERE n.x == {literal} RETURN n.id AS id ORDER BY id ASC') == [{'id': 1}, {'id': 2}]
    finally:
        db.close()
    file = Path(root) / 'boundaries-full-id.tdb'
    triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False).close()
    shutil.copyfile(Path(__file__).parents[1] / 'fixtures/numeric_ids/full_u64_v3.bin', str(file)+'.wal')
    db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
    try:
        before = Path(str(file)+'.wal').read_bytes()
        assert rows(db, 'MATCH (n {id:-3}) RETURN n') == []
        assert db.tql_mut('MATCH (n {id:-3}) SET n.mark == false')['affected'] == 0
        assert Path(str(file)+'.wal').read_bytes() == before
        db.tql_mut('MATCH (n {id:18446744073709551613}) SET n.mark == true')
        assert rows(db, 'MATCH (n {id:18446744073709551613}) RETURN n.id AS id, n.nested AS nested, n.mark AS mark') == [{'id': 18446744073709551613, 'nested': {'large': 18446744073709551613}, 'mark': True}]
    finally:
        db.close()
    db = open_db('u64', dtype='u64')
    try:
        db.insert_with_id(1, [16777216], {}); db.insert_with_id(2, [16777217], {})
        for query in ['SEARCH VECTOR [16777217] TOP 1 AS n WITH n RETURN n.id AS id', 'MATCH (n) RANK n BY VECTOR [16777217] TOP 1 RETURN n.id AS id']:
            assert rows(db, query) == [{'id': 2}]
        prepared = db.prepare_tql('SEARCH VECTOR [$q] TOP 1 AS n WITH n RETURN n.id AS id')
        assert [row.row for row in db.execute_prepared_tql(prepared, {'q': 16777217})] == [{'id': 2}]
        db.tql_mut('MATCH (n {id:1}) SET VECTOR(n) == [18446744073709551614]')
        db.tql_mut('MATCH (n {id:2}) SET VECTOR(n) == [18446744073709551615]')
        assert rows(db, 'SEARCH VECTOR [18446744073709551614] TOP 1 AS n WITH n RETURN n.id AS id') == [{'id': 1}]
        for literal in ['-1', '1.5', '18446744073709551616']:
            rejects(lambda: db.tql(f'SEARCH VECTOR [{literal}] TOP 1 RETURN *'))
    finally:
        db.close()
