import unittest

from fees import fee, surcharge


class FeeTest(unittest.TestCase):
    def test_fee_adds_the_weight(self):
        self.assertEqual(fee(2), 5)


class SurchargeTest(unittest.TestCase):
    def test_up_to_the_limit(self):
        self.assertEqual(surcharge(1), 1)
        self.assertEqual(surcharge(100), 1)

    def test_over_the_limit(self):
        self.assertEqual(surcharge(101), 5)


if __name__ == "__main__":
    unittest.main()
