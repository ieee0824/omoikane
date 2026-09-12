import unittest

from scripts.check_rustfmt import has_abort_marker


class RustfmtFailureDetectionTests(unittest.TestCase):
    def test_detects_abort_even_when_cargo_fmt_reports_success(self):
        self.assertTrue(has_abort_marker("memory allocation of 7236708580 bytes failed"))
        self.assertTrue(has_abort_marker("process terminated by signal: 6"))

    def test_accepts_normal_formatter_output(self):
        self.assertFalse(has_abort_marker(""))
        self.assertFalse(has_abort_marker("formatting completed"))


if __name__ == "__main__":
    unittest.main()
