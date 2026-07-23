# The shared lexical half of the source guards (scripts/check-sql.pl,
# scripts/check-r2.pl): blanking comment and string bodies so the
# guards' regexes can match across lines without a comment hiding a
# call, faking one, or swallowing the code after a URL. One copy on
# purpose - the blanker is what makes the guards evasion-resistant, and
# two drifting copies would be an evasion vector of their own.

use strict;
use warnings;

# The source with every comment and string body blanked, newlines kept
# (so reported line numbers still point at the source). Rust's block
# comments nest, and a `//` or `/*` inside a string starts nothing - so
# this walks the file rather than running a regex over it.
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

1;
