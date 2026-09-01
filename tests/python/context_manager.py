import os
import tempfile
import unittest

import triviumdb


class ContextManagerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.directory.cleanup()

    def path(self, name):
        return os.path.join(self.directory.name, name)

    def test_normal_exit_releases_writer_lock_and_persists(self):
        for dtype, vector in (
            ("f32", [1.0, 0.0]),
            ("f16", [1.0, 0.0]),
            ("u64", [1, 0]),
        ):
            path = self.path(f"normal-{dtype}.tdb")
            with triviumdb.TriviumDB(path, dim=2, dtype=dtype) as db:
                db.insert(vector, {"dtype": dtype})
            with triviumdb.TriviumDB(path, dim=2, dtype=dtype) as reopened:
                self.assertEqual(len(reopened), 1)

    def test_exceptional_exit_releases_writer_lock_and_preserves_exception(self):
        path = self.path("exception.tdb")
        with self.assertRaisesRegex(ValueError, "sentinel"):
            with triviumdb.TriviumDB(path, dim=2) as db:
                db.insert([1.0, 0.0], {"value": 1})
                raise ValueError("sentinel")
        with triviumdb.TriviumDB(path, dim=2) as reopened:
            self.assertEqual(len(reopened), 1)

    def test_closed_object_rejects_operations(self):
        path = self.path("closed.tdb")
        with triviumdb.TriviumDB(path, dim=2) as db:
            db.insert([1.0, 0.0], {})
        with self.assertRaisesRegex(RuntimeError, "Database closed|数据库已关闭"):
            db.search([1.0, 0.0], top_k=1)

    def test_manual_close_releases_lock(self):
        path = self.path("manual.tdb")
        db = triviumdb.TriviumDB(path, dim=2)
        db.close()
        reopened = triviumdb.TriviumDB(path, dim=2)
        reopened.close()

    def test_manual_close_inside_context_is_idempotent(self):
        path = self.path("idempotent.tdb")
        with triviumdb.TriviumDB(path, dim=2) as db:
            db.close()
            db.close()
        with triviumdb.TriviumDB(path, dim=2):
            pass

    def test_graph_expansion_controls_reach_bindings(self):
        path = self.path("graph-controls.tdb")
        with triviumdb.TriviumDB(path, dim=2) as db:
            seed = db.insert([1.0, 0.0], {"name": "seed"})
            strong = db.insert([0.0, 1.0], {"name": "strong"})
            weak = db.insert([-1.0, 0.0], {"name": "weak"})
            incoming = db.insert([0.0, -1.0], {"name": "incoming"})
            db.link(seed, strong, "allowed", 0.9)
            db.link(seed, weak, "allowed", 0.1)
            db.link(incoming, seed, "allowed", 0.8)
            outgoing = db.search_grouped(
                [1.0, 0.0],
                top_k=1,
                recall_k=1,
                rerank_k=1,
                expand_depth=1,
                min_score=0.9,
                max_edges_per_node=1,
                min_edge_weight=0.5,
                edge_direction="out",
            )
            self.assertIn(strong, [hit.id for hit in outgoing.graph_hits])
            self.assertNotIn(weak, [hit.id for hit in outgoing.graph_hits])
            incoming_hits = db.search_grouped(
                [1.0, 0.0],
                top_k=1,
                recall_k=1,
                rerank_k=1,
                expand_depth=1,
                min_score=0.9,
                edge_direction="in",
            )
            self.assertIn(incoming, [hit.id for hit in incoming_hits.graph_hits])

    def test_graph_expansion_rejects_invalid_controls(self):
        path = self.path("graph-invalid.tdb")
        with triviumdb.TriviumDB(path, dim=2) as db:
            db.insert([1.0, 0.0], {})
            with self.assertRaises(ValueError):
                db.search_advanced([1.0, 0.0], min_edge_weight=-0.1)
            with self.assertRaises(ValueError):
                db.search_advanced([1.0, 0.0], edge_direction="sideways")


if __name__ == "__main__":
    unittest.main()
