from pathlib import Path
import os
import tempfile
import unittest
from trustgraph_actions import endpoint
from check_composio_github import private_key


class ActionClientBoundaryTests(unittest.TestCase):
    def test_credentials_cannot_be_sent_to_nonlocal_http_or_a_url_with_credentials(self):
        for url in ["http://example.com", "http://127.0.0.1.evil.example", "https://user:password@example.com", "https://example.com/?key=x", "https://example.com/#key", "https://example.com/path"]:
            with self.assertRaises(ValueError):
                endpoint(url)
        self.assertEqual(endpoint("https://demo.example/"), "https://demo.example")
        self.assertEqual(endpoint("http://127.0.0.1:8085"), "http://127.0.0.1:8085")

    def test_gateway_key_file_must_be_private_and_not_a_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "key"
            path.write_text("example-test-key\n")
            path.chmod(0o600)
            self.assertEqual(private_key(path), "example-test-key")
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                private_key(path)
            path.chmod(0o600)
            link = Path(directory) / "link"
            os.symlink(path, link)
            with self.assertRaises(ValueError):
                private_key(link)


if __name__ == "__main__":
    unittest.main()
