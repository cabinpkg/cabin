#!/bin/sh

test_description='Test the test command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin test basic functionality' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new test_project &&
    cd test_project &&

    # Add a simple test to the main.cc file
    cat >src/main.cc <<-EOF &&
#include <iostream>

#ifdef CABIN_TEST
void test_addition() {
    int result = 2 + 2;
    if (result != 4) {
        std::cerr << "Test failed: 2 + 2 = " << result << ", expected 4" << std::endl;
        std::exit(1);
    }
    std::cout << "test test addition ... ok" << std::endl;
}

int main() {
    test_addition();
    return 0;
}
#else
int main() {
    std::cout << "Hello, world!" << std::endl;
    return 0;
}
#endif
EOF

    "$CABIN" test 1>stdout 2>stderr &&
    (
        test_path_is_dir cabin-out &&
        test_path_is_dir cabin-out/test &&
        test_path_is_dir cabin-out/test/unittests
    ) &&
    grep -q "test addition.*ok" stdout &&
    grep -q "1 passed; 0 failed" stderr
'

test_expect_success 'cabin test --help shows coverage option' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new test_project &&
    cd test_project &&
    "$CABIN" test --help >help_output 2>&1 &&
    grep -q -- "--coverage" help_output &&
    grep -q "Enable code coverage analysis" help_output
'

test_expect_success 'cabin test --coverage generates coverage files' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new coverage_project &&
    cd coverage_project &&

    # Add a simple test
    cat >src/main.cc <<-EOF &&
#include <iostream>

#ifdef CABIN_TEST
void test_function() {
    std::cout << "test coverage function ... ok" << std::endl;
}

int main() {
    test_function();
    return 0;
}
#else
int main() {
    std::cout << "Hello, world!" << std::endl;
    return 0;
}
#endif
EOF

    "$CABIN" test --coverage 1>stdout 2>stderr &&

    # Check that coverage files were generated
    find cabin-out/test -name "*.gcda" | head -1 | grep -q "\.gcda$" &&
    find cabin-out/test -name "*.gcno" | head -1 | grep -q "\.gcno$" &&

    # Check test output
    grep -q "coverage function.*ok" stdout &&
    grep -q "1 passed; 0 failed" stderr
'

test_expect_success 'cabin test --coverage uses coverage flags in compilation' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new verbose_project &&
    cd verbose_project &&

    # Add a simple test
    cat >src/main.cc <<-EOF &&
#include <iostream>

#ifdef CABIN_TEST
int main() {
    std::cout << "test verbose compilation ... ok" << std::endl;
    return 0;
}
#else
int main() {
    std::cout << "Hello, world!" << std::endl;
    return 0;
}
#endif
EOF

    # Clear any existing build artifacts to force recompilation
    rm -rf cabin-out &&

    "$CABIN" test --coverage -vv 1>stdout 2>stderr &&
    test_when_finished "rm -rf cabin-out/coverage" &&

    # Check that --coverage flag appears in compilation commands
    grep -q -- "--coverage" stdout &&

    # Check test passes
    grep -q "verbose compilation.*ok" stdout &&
    grep -q "1 passed; 0 failed" stderr
'

test_expect_success 'cabin test without --coverage does not generate coverage files' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new no_coverage_project &&
    cd no_coverage_project &&

    # Add a simple test
    cat >src/main.cc <<-EOF &&
#include <iostream>

#ifdef CABIN_TEST
int main() {
    std::cout << "test no coverage ... ok" << std::endl;
    return 0;
}
#else
int main() {
    std::cout << "Hello, world!" << std::endl;
    return 0;
}
#endif
EOF

    "$CABIN" test 1>stdout 2>stderr &&

    # Check that no coverage files were generated in a clean test
    # (Note: there might be some from previous tests, so we check that coverage files are not created)
    test $(find cabin-out/test -name "*.gcda" | wc -l) -eq 0 &&

    # Check test passes
    grep -q "no coverage.*ok" stdout &&
    grep -q "1 passed; 0 failed" stderr
'

test_done
