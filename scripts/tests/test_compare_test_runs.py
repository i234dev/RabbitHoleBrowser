"""Checks for incomplete or interleaved libtest evidence."""
import importlib.util
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "compare_test_runs", Path(__file__).parents[1] / "compare-test-runs.py"
)
COMPARATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COMPARATOR)


class TestOutcomeEvidence(unittest.TestCase):
    def outcomes(self, text):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "tests.log"
            path.write_text(text)
            return COMPARATOR.outcomes(path)

    def test_duplicate_names_and_ignored_tests_remain_counted(self):
        result = self.outcomes(
            "test shared_name ... ok\n"
            "test skipped ... ignored, requires display\n"
            "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;\n"
            "test shared_name ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n"
        )
        self.assertEqual(result[("shared_name", "ok")], 2)
        self.assertEqual(result[("skipped", "ignored")], 1)

    def test_native_stderr_splitting_a_test_line_is_incomplete(self):
        with self.assertRaisesRegex(ValueError, "summary totals"):
            self.outcomes(
                "test complete ... ok\n"
                "test player::tests::av1_decoder_opens ... Overrun in OBU bit buffer\n"
                "ok\n"
                "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n"
            )

    def test_missing_and_failed_summaries_are_rejected(self):
        for summary in ("", "test result: FAILED. 1 passed; 1 failed; 0 ignored;\n"):
            with self.subTest(summary=summary), self.assertRaisesRegex(ValueError, "summaries"):
                self.outcomes("test complete ... ok\n" + summary)

    def test_omitted_ignored_test_is_incomplete(self):
        with self.assertRaisesRegex(ValueError, "summary totals"):
            self.outcomes(
                "test complete ... ok\n"
                "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;\n"
            )


if __name__ == "__main__":
    unittest.main()
