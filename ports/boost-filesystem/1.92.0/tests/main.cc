// Exercises Boost.Filesystem's compiled path and operations code in a
// fresh directory under the system temporary directory, removed at exit.
#include <boost/filesystem.hpp>
#include <boost/filesystem/fstream.hpp>

namespace fs = boost::filesystem;

static int run(const fs::path& root) {
    const fs::path file = root / "sub" / "a.txt";
    if (!fs::create_directories(file.parent_path())) return 1;
    {
        fs::ofstream out(file);
        out << "hello";
    }
    if (!fs::is_regular_file(file) || fs::file_size(file) != 5) return 1;

    fs::copy_file(file, root / "b.txt");
    fs::rename(root / "b.txt", root / "c.txt");
    if (fs::exists(root / "b.txt") || fs::file_size(root / "c.txt") != 5) return 1;

    int entries = 0;
    for (fs::recursive_directory_iterator it(root), end; it != end; ++it) ++entries;
    if (entries != 3) return 1;  // sub, sub/a.txt, c.txt

    boost::system::error_code ec;
    fs::file_size(root / "missing", ec);
    if (!ec) return 1;
    try {
        fs::file_size(root / "missing");
        return 1;
    } catch (const fs::filesystem_error& e) {
        if (e.path1() != root / "missing") return 1;
    }

    return fs::remove_all(root) == 4 && !fs::exists(root) ? 0 : 1;
}

int main() {
    const fs::path p("dir/sub/../name.txt");
    if (p.filename() != "name.txt" || p.stem() != "name" || p.extension() != ".txt") return 1;
    if (p.lexically_normal().generic_string() != "dir/name.txt") return 1;
    if (fs::path("a/b").lexically_relative("a") != "b") return 1;

    const fs::path root = fs::temp_directory_path() / fs::unique_path("cabin-fs-%%%%-%%%%-%%%%");
    const int result = run(root);
    boost::system::error_code ec;
    fs::remove_all(root, ec);
    return result;
}
