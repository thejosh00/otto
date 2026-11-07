def fee(weight):
    if weight == 0:
        return 0
    return 3 + weight


def surcharge(weight):
    if weight > 100:
        return 5
    return 1
