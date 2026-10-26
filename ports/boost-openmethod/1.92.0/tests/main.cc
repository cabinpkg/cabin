// Exercises Boost.OpenMethod: single and multiple dispatch through open methods,
// including next() to call the base overrider.
#include <boost/openmethod.hpp>
#include <boost/openmethod/initialize.hpp>
#include <memory>
#include <string>

struct Animal { virtual ~Animal() = default; };
struct Cat : Animal {};
struct Dog : Animal {};
struct Bulldog : Dog {};

BOOST_OPENMETHOD_CLASSES(Animal, Cat, Dog, Bulldog);

using boost::openmethod::virtual_ptr;

BOOST_OPENMETHOD(poke, (virtual_ptr<Animal>), std::string);
BOOST_OPENMETHOD_OVERRIDE(poke, (virtual_ptr<Cat>), std::string) { return "hiss"; }
BOOST_OPENMETHOD_OVERRIDE(poke, (virtual_ptr<Dog>), std::string) { return "bark"; }
BOOST_OPENMETHOD_OVERRIDE(poke, (virtual_ptr<Bulldog> dog), std::string) { return next(dog) + "+bite"; }

BOOST_OPENMETHOD(meet, (virtual_ptr<Animal>, virtual_ptr<Animal>), int);
BOOST_OPENMETHOD_OVERRIDE(meet, (virtual_ptr<Animal>, virtual_ptr<Animal>), int) { return 0; }
BOOST_OPENMETHOD_OVERRIDE(meet, (virtual_ptr<Dog>, virtual_ptr<Cat>), int) { return 1; }
BOOST_OPENMETHOD_OVERRIDE(meet, (virtual_ptr<Cat>, virtual_ptr<Dog>), int) { return 2; }

int main() {
    boost::openmethod::initialize();

    std::unique_ptr<Animal> cat(new Cat), dog(new Dog), bulldog(new Bulldog);
    if (poke(*cat) != "hiss") return 1;
    if (poke(*dog) != "bark") return 1;
    if (poke(*bulldog) != "bark+bite") return 1;

    if (meet(*dog, *cat) != 1) return 1;
    if (meet(*cat, *bulldog) != 2) return 1;
    if (meet(*cat, *cat) != 0) return 1;
    return 0;
}
