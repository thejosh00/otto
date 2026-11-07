import unittest

from fees import fee


class FeeTest(unittest.TestCase):
    def test_fee_adds_the_weight(self):
        self.assertEqual(fee(2), 5)


if __name__ == "__main__":
    unittest.main()
