from pathlib import Path
import os
import tempfile
import unittest
from unittest.mock import patch
from trustgraph_actions import endpoint
from check_composio_github import private_key
from composio_connect import Connect, ISSUE_TOOL, preflight, schema_hash


class ActionClientBoundaryTests(unittest.TestCase):
    def test_connect_preflight_reads_repository_and_pins_account_schema_without_write(self):
        schema = {"type": "object", "required": ["owner", "repo", "title"],
                  "properties": {k: {"type": "string"} for k in ["title", "body", "owner", "repo"]}}
        with patch("composio_connect.Connect") as constructor:
            client = constructor.return_value
            client.call.side_effect = [
                {"session": {"id": "returned-session"}, "toolkit_connection_statuses": [
                    {"toolkit": "github", "has_active_connection": True,
                     "accounts": [{"id": "github-test", "status": "ACTIVE"}]}]},
                {"tool_schemas": {ISSUE_TOOL: {"input_schema": schema}}},
                {"results": [{"tool_slug": "GITHUB_GET_A_REPOSITORY", "response": {
                    "successful": True, "data": {"full_name": "example/demo", "has_issues": True, "archived": False}}}]},
            ]
            binding = preflight("test-key", "example/demo")
            self.assertEqual(binding["connected_account_id"], "github-test")
            self.assertEqual(binding["tool_schema_hash"], schema_hash(schema))
            self.assertIsNone(binding["tool_version"])
            last = client.call.call_args.args
            self.assertEqual(last[1]["tools"], [{"tool_slug": "GITHUB_GET_A_REPOSITORY", "account": "github-test",
                                               "arguments": {"owner": "example", "repo": "demo"}}])
            self.assertEqual(last[1]["session_id"], "returned-session")
        client = object.__new__(Connect)
        with self.assertRaises(ValueError):
            client.call("COMPOSIO_MULTI_EXECUTE_TOOL", {"tools": [{"tool_slug": ISSUE_TOOL}]})

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
