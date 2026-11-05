def calc(items):
    return sum(i["price"] * i["qty"] for i in items)


def checkout(items):
    return {"total": calc(items)}
