import unittest

from fees import fee, surcharge


class FeeTest(unittest.TestCase):
    def test_fee_adds_the_weight(self):
        self.assertEqual(fee(2), 5)


class SurchargeTest(unittest.TestCase):
    def test_light_parcel(self):
        self.assertEqual(surcharge(1), 1)


if __name__ == "__main__":
    unittest.main()
