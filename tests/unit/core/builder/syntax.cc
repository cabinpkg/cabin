// std
#include <sstream>
#include <string>
#include <vector>

// external
#include <boost/ut.hpp>

// internal
#include <poac/core/builder/syntax.hpp>

// NOLINTNEXTLINE(bugprone-exception-escape)
auto main() -> int {
  using namespace std::literals::string_literals;
  using namespace poac;
  using namespace boost::ut;
  using namespace boost::ut::spec;

  namespace builder = core::builder;
  using boost::algorithm::join;
  using Vec = Vec<std::string>;

  const std::string longword = std::string(10, 'a');
  const std::string longwordwithspaces =
      std::string(5, 'a') + "$ " + std::string(5, 'a');
  const std::string indent = "    ";

  describe("test line word wrap") = [&] {
    it("test single long word") = [&] {
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.line(longword);
      expect(eq(longword + '\n', writer.get_value()));
    };

    it("test few long words") = [&] {
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.line(join(Vec{"x"s, longword, "y"s}, " "));
      expect(
          eq(join(Vec{"x"s, indent + longword, indent + "y"}, " $\n") + '\n',
             writer.get_value())
      );
    };

    it("test comment wrap") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.comment("Hello /usr/local/build-tools/bin");
      expect(eq("# Hello\n# /usr/local/build-tools/bin\n"s, writer.get_value()))
          << "Filenames should not be wrapped";
    };

    it("test short words indented") = [] {
      // Test that indent is taking into account when breaking subsequent lines.
      // The second line should not be '    to tree', as that's longer than the
      // test layout width of 8.
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.line("line_one to tree");
      expect(
          eq("line_one $\n"
             "    to $\n"
             "    tree\n"s,
             writer.get_value())
      );
    };

    it("test few long words indented") = [&] {
      // Check wrapping in the presence of indenting.
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.line(join(Vec{"x"s, longword, "y"s}, " "), 1);
      expect(eq(
          join(
              Vec{"  "s + "x", "  " + indent + longword, "  " + indent + "y"},
              " $\n"
          ) + '\n',
          writer.get_value()
      ));
    };

    it("test escaped spaces") = [&] {
      builder::syntax::Writer writer{std::ostringstream(), 8};
      writer.line(join(Vec{"x"s, longwordwithspaces, "y"s}, " "));
      expect(
          eq(join(Vec{"x"s, indent + longwordwithspaces, indent + "y"}, " $\n")
                 + '\n',
             writer.get_value())
      );
    };

    it("test fit many words") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 78};
      writer.line(
          "command = cd ../../chrome; python ../tools/grit/grit/format/repack.py ../out/Debug/obj/chrome/chrome_dll.gen/repack/theme_resources_large.pak ../out/Debug/gen/chrome/theme_resources_large.pak",
          1
      );
      expect(eq(
          "  command = cd ../../chrome; python ../tools/grit/grit/format/repack.py $\n"
          "      ../out/Debug/obj/chrome/chrome_dll.gen/repack/theme_resources_large.pak $\n"
          "      ../out/Debug/gen/chrome/theme_resources_large.pak\n"s,
          writer.get_value()
      ));
    };

    it("test leading space") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 14};
      writer.variable("foo", Vec{"", "-bar", "-somethinglong"}, 0);
      expect(
          eq("foo = -bar $\n"
             "    -somethinglong\n"s,
             writer.get_value())
      );
    };

    it("test embedded dollar dollar") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 15};
      writer.variable("foo", Vec{"a$$b", "-somethinglong"}, 0);
      expect(
          eq("foo = a$$b $\n"
             "    -somethinglong\n"s,
             writer.get_value())
      );
    };

    it("test two embedded dollar dollars") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 17};
      writer.variable("foo", Vec{"a$$b", "-somethinglong"}, 0);
      expect(
          eq("foo = a$$b $\n"
             "    -somethinglong\n"s,
             writer.get_value())
      );
    };

    it("test leading dollar dollar") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 14};
      writer.variable("foo", Vec{"$$b", "-somethinglong"}, 0);
      expect(
          eq("foo = $$b $\n"
             "    -somethinglong\n"s,
             writer.get_value())
      );
    };

    it("test trailing dollar dollar") = [] {
      builder::syntax::Writer writer{std::ostringstream(), 14};
      writer.variable("foo", Vec{"a$$", "-somethinglong"}, 0);
      expect(
          eq("foo = a$$ $\n"
             "    -somethinglong\n"s,
             writer.get_value())
      );
    };
  };

  describe("test build") = [] {
    it("test variables dict") = [] {
      builder::syntax::Writer writer{std::ostringstream()};
      writer.build(
          {"out"}, "cc",
          builder::syntax::BuildSet{
              .inputs = Vec{"in"s},
              .variables = HashMap<String, String>{{"name", "value"}}
          }
      );

      expect(
          eq("build out: cc in\n"
             "  name = value\n"s,
             writer.get_value())
      );
    };

    it("test implicit outputs") = [] {
      builder::syntax::Writer writer{std::ostringstream()};
      writer.build(
          {"o"}, "cc",
          builder::syntax::BuildSet{
              .inputs = Vec{"i"s},
              .implicit_outputs = "io",
          }
      );

      expect(eq("build o | io: cc i\n"s, writer.get_value()));
    };
  };

  describe("test expand") = [] {
    it("test basic") = [] {
      const builder::syntax::Variables vars{{"x", "X"}};
      expect(eq("foo"s, builder::syntax::expand("foo", vars)));
    };

    it("test var") = [] {
      const builder::syntax::Variables vars{{"xyz", "XYZ"}};
      expect(eq("fooXYZ"s, builder::syntax::expand("foo$xyz", vars)));
    };

    it("test vars") = [] {
      const builder::syntax::Variables vars{{"x", "X"}, {"y", "YYY"}};
      expect(eq("XYYY"s, builder::syntax::expand("$x$y", vars)));
    };

    it("test space") = [] {
      const builder::syntax::Variables vars{};
      expect(eq("x y z"s, builder::syntax::expand("x$ y$ z", vars)));
    };

    it("test locals") = [] {
      const builder::syntax::Variables vars{{"x", "a"}};
      const builder::syntax::Variables local_vars{{"x", "b"}};
      expect(eq("a"s, builder::syntax::expand("$x", vars)));
      expect(eq("b"s, builder::syntax::expand("$x", vars, local_vars)));
    };

    it("test double") = [] {
      expect(eq("a b$c"s, builder::syntax::expand("a$ b$$c", {})));
    };
  };
}
