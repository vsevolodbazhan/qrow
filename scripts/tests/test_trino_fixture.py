"""The Trino fixture starts tests only after a query reads its task results."""
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("trino_fixture", ROOT / "scripts/e2e/trino.py")
trino = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trino)


class Response(io.BytesIO):
    def __init__(self, body):
        super().__init__(json.dumps(body).encode())


class ReadinessTests(unittest.TestCase):
    def ready(self, *pages):
        responses = iter(pages)
        requests = []

        def urlopen(request, **_):
            requests.append(request)
            page = next(responses)
            if isinstance(page, Exception):
                raise page
            return Response(page)

        with patch.object(trino.urllib.request, "urlopen", side_effect=urlopen):
            return trino.accepts_queries("https://localhost:1", None), requests

    def test_rows_on_an_earlier_page_than_the_empty_last_page(self):
        ready, requests = self.ready({"starting": False}, {"nextUri": "https://localhost:1/a"},
                                     {"data": [[5]], "nextUri": "https://localhost:1/b"}, {"stats": {"state": "FINISHED"}})
        self.assertTrue(ready)
        self.assertEqual([getattr(r, "full_url", r) for r in requests][2:], ["https://localhost:1/a", "https://localhost:1/b"])
        self.assertTrue(all(r.get_header("Authorization") for r in requests[1:]))

    def test_starting_coordinator_failed_query_and_wrong_rows_are_not_ready(self):
        self.assertFalse(self.ready({"starting": True})[0])
        self.assertFalse(self.ready({"starting": False}, {"nextUri": "https://localhost:1/a"},
                                    {"error": {"message": "Trino server is still initializing"}})[0])
        self.assertFalse(self.ready({"starting": False}, {"data": [[4]]})[0])
        self.assertFalse(self.ready({"starting": False}, trino.urllib.error.URLError("503"))[0])


if __name__ == "__main__":
    unittest.main()
