#include <iostream>
#include <string>
#include <vector>

struct Item {
    std::string name;
    int weight;
};

int main() {
    std::vector<Item> items{{"syntax", 3}, {"diff", 5}};

    for (const auto &item : items) {
        std::cout << item.name << " -> " << (item.weight * 10) << '\n';
    }

    return 0;
}
