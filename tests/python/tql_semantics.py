"""共享查询返回契约与 TQL 写入边界，通过实际安装的原生模块执行。"""
import json
from pathlib import Path
import triviumdb


def run_tql_semantics(root: str) -> None:
    contract = json.loads((Path(__file__).parents[1] / 'contracts/tql_return_cases.json').read_text(encoding='utf-8'))
    def open_db(name: str, **options):
        return triviumdb.TriviumDB(str(Path(root) / f'semantics-{name}.tdb'), dim=contract['dimension'], auto_build_quiver=False, **options)
    def rows(db, query):
        return [row.row for row in db.tql(query)]
    db = open_db('returns')
    try:
        for row in contract['records']:
            db.insert_with_id(row['id'], [1], row['payload'])
        db.insert_with_id(99, [0], {'kind': 'root'})
        for row in contract['records']:
            db.link(99, row['id'], 'item', row['id'])
        plan = rows(db, 'EXPLAIN SEARCH VECTOR [1] TOP 1 RETURN *')[0]['plan']['payload']
        assert plan['vector_access_path'] == 'exact_fallback'
        assert plan['access_path']['kind'] == 'full_node_scan'
        for source in contract['sources']:
            for case in contract['cases']:
                query = source['query'] + ' ' + case['suffix'].replace('{var}', source['variable'])
                assert rows(db, query) == case['rows'], query
            n = source['variable']
            assert rows(db, f'{source["query"]} RETURN {n}.g AS g, COUNT(*) AS {n} ORDER BY {n}.g ASC') == [{'g': 'a', n: 1}, {'g': 'b', n: 2}]
    finally:
        db.close()
    db = open_db('indexed-order')
    try:
        for record in contract['indexed_order']['records']:
            db.insert_with_id(record['id'], [1], record['payload'])
        for target in [2, 3]:
            db.link(1, target, 'related', 3.4028234663852886e38)
        for phase in range(len(contract['indexed_order']['indexes']) + 1):
            if phase > 0:
                db.create_ordered_index(contract['indexed_order']['indexes'][phase - 1])
            for case in contract['indexed_order']['cases']:
                assert rows(db, case['query']) == case['rows'], case['query']
    finally:
        db.close()
    db = open_db('set')
    try:
        db.insert_with_id(1, [1], {'a': 0, 'b': 0, 'keep': True})
        assert db.tql_mut('MATCH (n) SET n.a == 1, n.b == 2, n.a == 3')['affected'] == 1
        assert db.get_payload(1) == {'a': 3, 'b': 2, 'keep': True}
    finally:
        db.close()
    db = open_db('set')
    try:
        assert db.get_payload(1) == {'a': 3, 'b': 2, 'keep': True}
    finally:
        db.close()
    db = open_db('vector', dtype='u64')
    try:
        db.insert_with_id(1, [7], {})
        for value in [16777217, 2**53+1, 2**64-1]:
            db.tql_mut(f'MATCH (n) SET VECTOR(n) == [{value}]')
            assert list(db.get(1).vector) == [value]
        wal_path = Path(root) / 'semantics-vector.tdb.wal'
        before = wal_path.read_bytes()
        for value in ['-1', '1.5', '18446744073709551616']:
            try:
                db.tql_mut(f'MATCH (n) SET VECTOR(n) == [{value}]')
            except (ValueError, RuntimeError):
                pass
            else:
                raise AssertionError('非法 u64 向量必须拒绝')
            assert list(db.get(1).vector) == [2**64-1]
            assert wal_path.read_bytes() == before
    finally:
        db.close()
    db = open_db('budget', max_query_rows=2, memory_limit_mb=1)
    try:
        for ident in range(1, 4):
            db.insert_with_id(ident, [1], {'x': ident})
        for source in ['MATCH (n)', 'MATCH (n) AS n WITH n']:
            for query in [f'{source} RETURN n', f'{source} RETURN n LIMIT 2 OFFSET 10000']:
                try:
                    db.tql(query)
                except RuntimeError:
                    pass
                else:
                    raise AssertionError(f'查询预算必须生效：{query}')
            assert rows(db, f'{source} RETURN n.id AS id LIMIT 3') == [{'id': 1}, {'id': 2}, {'id': 3}]
    finally:
        db.close()
    db = open_db('create', max_query_rows=1)
    try:
        assert db.tql_mut('MATCH (n {kind:"seed"}) CREATE (m {kind:"copy"})') == {'affected': 0, 'created_ids': []}
        for ident in range(1, 4):
            db.insert_with_id(ident, [1], {'kind': 'seed'})
        assert db.tql_mut('MATCH (n {kind:"seed"}) CREATE (n)-[:copy]->(m {kind:"copy"})') == {'affected': 6, 'created_ids': [4, 5, 6]}
    finally:
        db.close()
    db = open_db('create')
    try:
        assert rows(db, 'MATCH (n)-[e:copy]->(m) RETURN n.id AS source, m.id AS target ORDER BY n.id ASC') == [{'source': 1, 'target': 4}, {'source': 2, 'target': 5}, {'source': 3, 'target': 6}]
    finally:
        db.close()
