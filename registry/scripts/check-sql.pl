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

# The source with every comment and string body blanked, newlines kept
# (so reported line numbers still point at the source). Rust's block
# comments nest, and a `//` or `/*` inside a string starts nothing - so
# this walks the file rather than running a regex over it, and the
# checks below can then match across lines without a comment hiding a
# call, faking one, or swallowing the code after a URL.
sub blank_comments_and_strings {
    my ($src) = @_;
    my ($out, $i, $n) = ('', 0, length $src);
    while ($i < $n) {
        my $two = substr($src, $i, 2);
        if ($two eq '//') {
            my $end = index($src, "\n", $i);
            $end = $n if $end < 0;
            $out .= ' ' x ($end - $i);
            $i = $end;
        } elsif ($two eq '/*') {
            my ($start, $depth) = ($i, 0);
            while ($i < $n) {
                my $here = substr($src, $i, 2);
                if ($here eq '/*') { $depth++; $i += 2; }
                elsif ($here eq '*/') { $depth--; $i += 2; last if $depth == 0; }
                else { $i++; }
            }
            $out .= (substr($src, $start, $i - $start) =~ s/[^\n]/ /gr);
        } elsif (substr($src, $i) =~ m{^r(\#*)"}) {
            # A raw string ends at the quote followed by as many hashes
            # as it opened with.
            my $hashes = $1;
            my $opened = length($hashes) + 2;
            my $close = index($src, '"' . $hashes, $i + $opened);
            $close = $close < 0 ? $n : $close + 1 + length $hashes;
            $out .= (substr($src, $i, $close - $i) =~ s/[^\n]/ /gr);
            $i = $close;
        } elsif (substr($src, $i, 1) eq '"') {
            my $j = $i + 1;
            while ($j < $n) {
                my $c = substr($src, $j, 1);
                last if $c eq '"';
                $j += ($c eq '\\') ? 2 : 1;
            }
            $j = $j < $n ? $j + 1 : $n;
            $out .= (substr($src, $i, $j - $i) =~ s/[^\n]/ /gr);
            $i = $j;
        } elsif (substr($src, $i) =~ m{^(b?'(?:\\.|[^'\\\n])')}) {
            # A character literal - `'"'` must not open a string. A
            # lifetime (`&'a str`) has no closing quote and falls
            # through to the ordinary branch below.
            $out .= (' ' x length $1);
            $i += length $1;
        } else {
            $out .= substr($src, $i, 1);
            $i++;
        }
    }
    return $out;
}

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
}
exit($fail ? 1 : 0);
