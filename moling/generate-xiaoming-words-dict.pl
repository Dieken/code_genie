#!/usr/bin/env perl
#
# Usage:
#   perl -CSDA -lanE 'print "$F[0]" if !exists $h{$F[0]} && /^\p{Han}{2,}\t/; $h{$F[0]}=1' \
#       星陳輸入法_v3.12.0/schema/yuhao/yustar{_sc,,_tc}.words.dict.yaml |
#       ./generate-xiaoming-words-dict.pl --trailing > xiaoming-full-words.txt
#

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
my $is_trailing = 0;    # 是否使用每字的末根

GetOptions(
    "chaifen=s"     => \$chaifen_file,
    "roots=s"       => \$roots_file,
    "trailing!"     => \$is_trailing,
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
# 完整版词规则，科测 6w 词词重 5.198%(trailing=0), 4.039%(trailing=1)：
#
# 二字词：
#     首字: 首根 AB  [次根 A | 末根 A]
#     次字单根：首根 AB
#     次字多根：首根 A  <次根 AB | 末根 AB>
# 三字词：
#     首字：<首根 AB | 末根 AB>
#     次字：<首根 A | 末根 A>
#     末字单根：首根 AB
#     末字多根：首根 A  <次根 A | 末根 A>
# 四字及以上：
#     首字：<首根 AB | 末根 AB>
#     次字：<首根 A | 末根 A>
#     三字：<首根 A | 末根 A>
#     末字：<首根 A | 末根 A>
#
while (<>) {
    next if /^\s*#/ || /^\s*#/;
    chomp;

    my ($word, $freq) = split;
    next unless $word && $word =~ /^\p{Han}{2,}$/;

    my @cf = map { $chaifen{$_} } split //, $word;

    my $cf1 = $cf[0];
    my $code = @cf == 2 || ! $is_trailing ?
        substr($roots{ $cf1->[0] }, 0, 2) :
        substr($roots{ $cf1->[-1] }, 0, 2);

    if (@cf == 2) {
        my $cf2 = $cf[1];

        if (@$cf1 > 1) {
            if ($is_trailing) {
                $code .= substr($roots{ $cf1->[-1] }, 0, 1);
            } else {
                $code .= substr($roots{ $cf1->[1] }, 0, 1);
            }
        }

        if (@$cf2 == 1) {
            $code .= substr($roots{ $cf2->[0] }, 0, 2);
        } else {
            $code .= substr($roots{ $cf2->[0] }, 0, 1);

            if ($is_trailing) {
                $code .= substr($roots{ $cf2->[-1] }, 0, 5 - length($code));
            } else {
                $code .= substr($roots{ $cf2->[1] }, 0, 5 - length($code));
            }
        }
    } elsif (@cf == 3) {
        my ($cf2, $cf3) = @cf[1 .. 2];

        $code .= $is_trailing ?
            substr($roots{ $cf2->[-1] }, 0, 1) :
            substr($roots{ $cf2->[0] }, 0, 1);

        if (@$cf3 == 1) {
            $code .= substr($roots{ $cf3->[0] }, 0, 2);
        } else {
            $code .= substr($roots{ $cf3->[0] }, 0, 1);

            if ($is_trailing) {
                $code .= substr($roots{ $cf3->[-1] }, 0, 1);
            } else {
                $code .= substr($roots{ $cf3->[1] }, 0, 1);
            }
        }
    } else {
        if ($is_trailing) {
            $code .= join "", map { substr($roots{ $_->[-1] }, 0, 1) } (@cf[1 .. 2], $cf[-1]);
        } else {
            $code .= join "", map { substr($roots{ $_->[0] }, 0, 1) } (@cf[1 .. 2], $cf[-1]);
        }
    }

    if (defined $freq) {
        print "$word\t$code\t$freq\n";
    } else {
        print "$word\t$code\n";
    }
}
