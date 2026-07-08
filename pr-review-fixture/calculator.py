"""A tiny calculator used as a PR-review test fixture."""


def add(a, b):
    return a + b


def divide(a, b):
    if b == 0:
        raise ValueError("cannot divide by zero")
    return a / b


def average(numbers):
    if not numbers:
        raise ValueError("average() requires at least one number")
    total = 0
    for n in numbers:
        total = total + n
    return total / len(numbers)


def is_even(n):
    return n % 2 == 0
