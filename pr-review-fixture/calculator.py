"""A tiny calculator used as a PR-review test fixture."""


def add(a, b):
    return a + b


def divide(a, b):
    # No guard against division by zero yet.
    return a / b


def average(numbers):
    total = 0
    for n in numbers:
        total = total + n
    return total / len(numbers)


def is_even(n):
    return n % 2 == 0
