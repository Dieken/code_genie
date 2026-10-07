#!/usr/bin/env perl

# https://perldoc.perl.org/perluniintro#Perl's-Unicode-Support  v5.28
# https://perldoc.perl.org/feature#The-'signatures'-feature     v5.36
# https://perldoc.perl.org/perlunicook#℞-0:-Standard-preamble   v5.36

use v5.36;                              # or later to get "unicode_strings" feature, plus strict and warnings
use utf8;                               # so literals and identifiers can be in UTF-8
use warnings qw(FATAL utf8);            # fatalize encoding glitches
use open     qw(:std :encoding(UTF-8)); # undeclared streams in UTF-8
use Encode   qw(decode);
@ARGV = map { decode('UTF-8', $_, Encode::FB_CROAK) } @ARGV;

use Getopt::Long;
use autodie;

my $chaifen_file = "output-20261003-203525/source/chaifen-all.txt";
my $roots_file = "output-20261003-203525/thread-04/output-keymap.txt";
my $is_simple = 0;      # 是否使用简化的词编码

GetOptions(
    "chaifen=s"     => \$chaifen_file,
    "roots=s"       => \$roots_file,
    "simple!"       => \$is_simple,
) or die "Failed to parse command line!\n";

my %chaifen;
my %roots;


{
    open my $fh, "<", $chaifen_file;
    while (<$fh>) {
        chomp;

        my @a = split /\t/;
        my @b = split /\s+/, $a[1];

        $chaifen{$a[0]} = \@b;
    }
    close $fh;
}

{
    open my $fh, "<", $roots_file;
    while (<$fh>) {
        next if /^#/;
        chomp;

        my @a = split;
        $roots{$a[0]} = lc($a[1]);
    }
}


# https://github.com/Dieken/code_genie/commit/741a1571b37505806e4058c2c6952935f6aa57a5
#
# 完整版词规则：
#
# 二字词：
#     两字单根：AB AB -- AaAbBaBb
#     前单后二：AB AAB -- AaAbBaBcBd
#     前二后单：ABA AB -- AaAbAcBaBb
#     两字多根：ABA AA -- AaAbAcBaBc
# 三字词：
#     末字单根：AB A AB -- AaAbBaCaCb
#     末字多根：AB A AA -- AaAbBaCaCc
# 四字及以上：
#     AB A A A -- AaAbBaCaZa
#
#
# 简化版词规则:
#
# 二字词：
#   AB AB
#   AB AAB
# 三字词:
#   AB A AB
# 四字及以上：
#   AB A A A
while (<>) {
    next if /^\s*#/ || /^\s*#/;
    chomp;

    my ($word, $freq) = split;
    next unless $word && $word =~ /^\p{Han}{2,}$/;

    my @cf = map { $chaifen{$_} } split //, $word;

    my $cf1 = $cf[0];
    my $code = substr($roots{ $cf1->[0] }, 0, 2);

    if (@cf == 2) {
        my $cf2 = $cf[1];

        if (! $is_simple && @$cf1 > 1) {
            $code .= substr($roots{ $cf1->[1] }, 0, 1);
        }

        if (@$cf2 == 1) {
            $code .= substr($roots{ $cf2->[0] }, 0, 2);
        } else {
            $code .= substr($roots{ $cf2->[0] }, 0, 1);
            $code .= substr($roots{ $cf2->[1] }, 0, 5 - length($code));
        }
    } elsif (@cf == 3) {
        my ($cf2, $cf3) = @cf[1 .. 2];

        $code .= substr($roots{ $cf2->[0] }, 0, 1);

        if ($is_simple || @$cf3 == 1) {
            $code .= substr($roots{ $cf3->[0] }, 0, 2);
        } else {
            $code .= substr($roots{ $cf3->[0] }, 0, 1);
            $code .= substr($roots{ $cf3->[1] }, 0, 1);
        }
    } else {
        $code .= join "", map { substr($roots{ $_->[0] }, 0, 1) } (@cf[1 .. 2], $cf[-1]);
    }

    if (defined $freq) {
        print "$word\t$code\t$freq\n";
    } else {
        print "$word\t$code\n";
    }
}
