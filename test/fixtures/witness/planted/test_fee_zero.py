import unittest

from fees import fee


class ZeroTest(unittest.TestCase):
    def test_zero_weight_is_free(self):
        self.assertEqual(fee(0), 0)


if __name__ == "__main__":
    unittest.main()
