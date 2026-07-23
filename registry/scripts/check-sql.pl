#!/usr/bin/env perl
#
# The lexical half of scripts/check-sql.sh (see there, and
# docs/architecture.md, "Why no ORM"): every call that can reach
# D1Database::prepare must name a sql:: const, and D1's unprepared
# escape hatch (exec) is rejected outright. Reads the Rust sources named
# on the command line; prints every violation and exits non-zero if
# there is one.

use strict;
use warnings;
use FindBin;

# blank_comments_and_strings lives in the shared lexical.pm so this
# guard and scripts/check-r2.pl cannot drift apart on what counts as a
# comment or a string.
require "$FindBin::RealBin/lexical.pm";

my $fail = 0;
for my $file (@ARGV) {
    open my $handle, '<', $file or die "$file: $!\n";
    my $source = blank_comments_and_strings(do { local $/; <$handle> });
    # Any spelling that can reach the D1 methods - `.prepare(`,
    # `::prepare(`, `r#prepare`, the name split from its receiver or its
    # paren across lines. Only a call whose sole argument is a sql::
    # const passes, so a dynamic argument fails; the word boundary keeps
    # `prepare_statement` and `execute` out, and the lookahead for `(`
    # keeps plain field access (`config.exec`) out. The argument is read
    # through a lookahead, so an accepted call never consumes the one
    # behind it, and it is whitespace-normalized first, so wrapping the
    # call or commenting its argument stays acceptable.
    # The governor's Durable Object SQLite statements have their own
    # consolidated home with the same assurance model as sql.rs: every
    # statement is a module-local const in src/governor.rs, executed by
    # the engine and prepared against the real governor schema by its
    # host tests. So src/governor.rs may exec a bare SCREAMING_CASE
    # const, and src/governor_do.rs - the storage adapter the engine
    # runs through - may exec exactly its pass-through parameters
    # (`sql`, the engine's statement; `statement`, the schema loop) or
    # a named const; dynamic and literal spellings stay rejected even
    # there.
    my $is_governor    = $file =~ m{(?:^|/)src/governor\.rs$};
    my $is_governor_do = $file =~ m{(?:^|/)src/governor_do\.rs$};
    while ($source =~ m{ [.:] \s* (?: r\# )? (prepare|exec) \b (?= \s* \( ) (?= \s* (.{0,400}) ) }gsx) {
        my ($method, $argument) = ($1, $2);
        $argument =~ s/\s+/ /g;
        next if $method eq 'prepare' && $argument =~ m{^\( ?sql::[A-Z][A-Z0-9_]* ?,? ?\)};
        next if $method eq 'exec' && $is_governor && $argument =~ m{^\( ?[A-Z][A-Z0-9_]* ?,};
        next if $method eq 'exec'
            && $is_governor_do
            && $argument =~ m{^\( ?(?:sql|statement|[A-Z][A-Z0-9_]*) ?,};
        # The engine's host-test rusqlite adapter forwards the engine's
        # `sql` parameter verbatim; only that exact spelling passes.
        next if $method eq 'prepare' && $is_governor && $argument =~ m{^\( ?sql ?\)};
        my $line = 1 + (substr($source, 0, $-[0]) =~ tr/\n//);
        print "$file:$line: $method" . substr($argument, 0, 40) . "\n";
        $fail = 1;
    }
    # A path-form method item (`D1Database::prepare` with no call
    # parens) is an alias that would launder every later call past the
    # scan above, so creating one is itself a violation.
    while ($source =~ m{ :: \s* (?: r\# )? (prepare|exec) \b (?! \s* \( ) }gsx) {
        my $line = 1 + (substr($source, 0, $-[0]) =~ tr/\n//);
        print "$file:$line: $1 method alias (path form without a call); "
            . "call it directly instead\n";
        $fail = 1;
    }
}
exit($fail ? 1 : 0);
