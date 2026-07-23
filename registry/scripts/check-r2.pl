#!/usr/bin/env perl
#
# The lexical half of scripts/check-r2.sh (see there, and
# docs/architecture.md, "The cost governor"): every way the Worker can
# obtain an R2 bucket handle (`env.bucket(...)` in any spelling) must
# sit in a function that was reviewed to admit its billable calls
# through the governor first. The allowlist below pins those functions
# with their exact acquisition counts; a new site (or a new acquisition
# inside a pinned function) fails until the reviewer confirms the
# governor admission and re-pins it here. Reads the Rust sources named
# on the command line; prints every violation and exits non-zero if
# there is one.

use strict;
use warnings;
use FindBin;

require "$FindBin::RealBin/lexical.pm";

# (path suffix) => { enclosing fn => sanctioned acquisition count }.
# glue.rs: the four request-path acquisitions are each immediately
# preceded by a governor decide (artifact/read/publish paths), the
# reclaim delete is R2's one free operation, the heal and drain helpers
# admit per call inside, and the cron entry acquires both buckets once
# for the drain. web_glue's source viewer and backup_glue's dump job
# admit before every billable call (docs/architecture.md).
my %allow = (
    'src/glue.rs' => {
        artifact_response           => 1,
        charged_blob_read           => 1,
        persist_new_version         => 1,
        replace_rejected_version    => 1,
        delete_blob_if_unreferenced => 1,
        heal_blobs_on_retry         => 1,
        drain_backup_queue          => 2,
    },
    'src/web_glue.rs'    => { package_source   => 1 },
    'src/backup_glue.rs' => { run_nightly_dump => 1 },
);

my $fail = 0;
for my $file (@ARGV) {
    open my $handle, '<', $file or die "$file: $!\n";
    my $source = blank_comments_and_strings(do { local $/; <$handle> });
    my ($suffix) = grep { $file =~ m{(?:^|/)\Q$_\E$} } keys %allow;
    my $sanctioned = $suffix ? $allow{$suffix} : {};

    # Every `fn name` position, for attributing a call site to its
    # nearest preceding function. A closure inside a function
    # attributes to that function; good enough for a pin whose job is
    # to force review, not to prove admission.
    my @fns;
    while ($source =~ m{\bfn\s+(?:r\#)?([A-Za-z0-9_]+)}g) {
        push @fns, [ $-[0], $1 ];
    }

    my %seen;
    # Any spelling that can reach Env::bucket - `.bucket(`,
    # `::bucket(`, `r#bucket`, split across lines or comments. The
    # word boundary keeps `bucket_from_columns` out; the lookahead for
    # `(` keeps field access (`auth.bucket`) out.
    while ($source =~ m{ [.:] \s* (?: r\# )? bucket \b (?= \s* \( ) }gsx) {
        my $offset = $-[0];
        my ($enclosing) = grep { $_->[0] <= $offset } reverse @fns;
        my $fn = $enclosing ? $enclosing->[1] : '(no enclosing fn)';
        $seen{$fn}++;
        next if exists $sanctioned->{$fn} && $seen{$fn} <= $sanctioned->{$fn};
        my $line = 1 + (substr($source, 0, $offset) =~ tr/\n//);
        print "$file:$line: unsanctioned R2 bucket acquisition in $fn - "
            . "prove the governor admission and pin it in scripts/check-r2.pl\n";
        $fail = 1;
    }

    # A path-form method item (`Env::bucket` with no call parens) is an
    # alias that would launder every later acquisition past the scan
    # above, so creating one is itself a violation. The dotted form
    # needs no twin check: `env.bucket` without parens is not valid
    # Rust for a method value, and plain field access is another name.
    while ($source =~ m{ :: \s* (?: r\# )? bucket \b (?! \s* \( ) }gsx) {
        my $line = 1 + (substr($source, 0, $-[0]) =~ tr/\n//);
        print "$file:$line: R2 bucket method alias (path form without a call); "
            . "call it directly in a pinned function instead\n";
        $fail = 1;
    }

    # worker's generic `env.get_binding::<T>(name)` can also yield a
    # Bucket without the `bucket` token appearing at all, and an
    # unchecked JS cast can conjure one from any JsValue. Nothing in
    # this Worker needs either, so both are banned outright. (Reflect
    # over the raw env object could still do it - that is deliberate
    # evasion, which is code review's job, not a tripwire's.)
    while ($source =~ m{ (?: [.:] \s* )? (?: r\# )? (get_binding|unchecked_into) \b }gsx) {
        my $line = 1 + (substr($source, 0, $-[0]) =~ tr/\n//);
        print "$file:$line: $1 sidesteps the typed accessors "
            . "(an R2 handle without the bucket token); use the typed "
            . "Env method in a pinned function\n";
        $fail = 1;
    }

    # A pinned function that still exists but no longer acquires its
    # bucket means the seam moved; the pin must move with it or it
    # stops guarding. A pin whose function is gone entirely is left to
    # the review that removed the function.
    my %defined = map { $_->[1] => 1 } @fns;
    for my $fn (sort keys %{$sanctioned}) {
        next unless $defined{$fn};
        next if ($seen{$fn} // 0) == $sanctioned->{$fn};
        next if ($seen{$fn} // 0) > $sanctioned->{$fn};    # already reported
        print "$file: $fn is pinned for $sanctioned->{$fn} acquisition(s) "
            . "but has " . ($seen{$fn} // 0) . "; update scripts/check-r2.pl\n";
        $fail = 1;
    }
}
exit($fail ? 1 : 0);
