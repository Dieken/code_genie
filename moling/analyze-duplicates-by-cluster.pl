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
use List::Util qw/sum/;
use autodie;

my $roots_file = "roots.txt";
my $chaifen_file = "chaifen.txt";
my $cluster_file = "roots-cluster.txt";
my $max_dups = 9;

GetOptions(
    "roots=s"       => \$roots_file,
    "chaifen=s"     => \$chaifen_file,
    "cluster=s"     => \$cluster_file,      # file path or "root1 root2 ; root3 root4"
    "max-dups=i"    => \$max_dups,
);

my $roots = read_roots($roots_file);                # { root => sy }
my $chaifens = read_chaifens($chaifen_file);        # { char => {chaifen => @, weight => $} }
my $clusters = read_clusters($cluster_file);        # { char => dama_id }
my $total_weights = total_weights($chaifens);

dump_dups($total_weights, $clusters,
          calculate_dups($chaifens, $roots, $clusters));


########################################################################
sub read_roots($file) {
    my %roots;

    open my $fh, "<", $file;
    while (<$fh>) {
        next if /^\s*#/ || /^\s*$/;

        chomp;
        my @a = split;
        $roots{$a[0]} = $a[1];
    }
    close $fh;

    return \%roots;
}

sub read_chaifens($file) {
    my %chaifen;

    open my $fh, "<", $file;
    while (<$fh>) {
        next if /^\s*#/ || /^\s*$/;

        chomp;
        my @a = split /\t/;
        my @b = split /\s+/, $a[1];
        die "拆分不能超过四根： $_\n" if @b > 4;

        $chaifen{$a[0]} = {
            chaifen => \@b,
            weight  => $a[2],
        };
    }
    close $fh;

    return \%chaifen;
}

sub read_clusters($file) {
    my %clusters;

    if (-f $file) {
        open my $fh, "<", $file;
        while (<$fh>) {
            next if /^\s*#/ || /^\s*$/;

            chomp;
            my @a = split /\t/;
            my @b = split /\s+/, $a[0];
            for (@b) {
                $clusters{$_} = $. if $_;
            }
        }
        close $fh;
    } else {
        # 命令行直接指定, "root1 root2 ; root3 root4"
        my @a = split /[,;，；]/, $file;
        for (my $i = 0; $i < @a; ++$i) {
            my @b = split /\s+/, $a[$i];
            for (@b) {
                $clusters{$_} = $i if $_;
            }
        }
    }

    return \%clusters;
}

sub total_weights($chaifen) {
    return sum map { $_->{weight} } values %$chaifen;
}

sub calculate_dups($chaifens, $roots, $clusters) {
    my %codes;      # { char => @code }

    while (my ($char, $v) = each %$chaifens) {
        my $cf = $v->{chaifen};

        my @code;
        for (@$cf) {
            push @code, exists $clusters->{$_} ? $clusters->{$_} : "$_.A";
        }

        my $root = $cf->[@$cf - 1];
        if (@$cf == 2) {
            # 回头码: A1A2S2S1Y1
            push @code, substr($roots->{$root}, 0, 1) if length($roots->{$root}) > 1;
            $root = $cf->[0];
        }
        push @code, split(//, $roots->{$root});
        @code = @code[0..3] if @code > 4;
        push @code, "" if @code == 2;
        push @code, "" if @code == 3;
        $codes{$char} = \@code;
    }

    my @static_dups;            # [ static_dup... ]
    my @dynamic_dups;           # [ ynamic_dup... ]
    my @arr_arr_dup_chars;      # [ [ [ dup_char... ] ] ]

    # 统计一阶重码：
    #    0:  取 1, 2, 3 三码，检查重码情况，静态重码不应大于 20 (第 0 码最多有 20 种不同取值)
    #    1:  取 0, 2, 3 三码，检查重码情况，静态重码不应大于 25 (第 1 码最多有 25 种不同取值)
    #    2:  取 0, 1, 3 三码，检查重码情况，静态重码不应大于 25 (第 2 码最多有 25 种不同取值)
    #    3:  取 0, 1, 2 三码，检查重码情况，静态重码不应大于 25 (第 3 码最多有 25 种不同取值)
    #    4:  取 0, 1, 2, 4 四码，检查重码情况
    for my $i (0 .. 4) {
        my %dups;   # { "code0-code1-...-codeN" => char => 1 }

        $static_dups[$i] = 0;
        $dynamic_dups[$i] = 0;

        while (my ($char, $code) = each %codes) {
            my @code = @$code;
            if ($i < 4) {
                @code = @code[ grep { $_ != $i } (0 .. 3) ];
            }

            $dups{ join("-", @code) }{$char} = 1;
        }

        my @arr_dup_chars;      # [ [ dup_char... ]]
        for (keys %dups) {
            my @chars = keys %{ $dups{$_} };
            next if @chars == 1;

            $static_dups[$i] += @chars;
            push @arr_dup_chars, [ sort { $chaifens->{$b}{weight} <=> $chaifens->{$a}{weight} || $a cmp $b }  @chars ];
            for my $char (@chars) {
                $dynamic_dups[$i] += $chaifens->{$char}{weight};
            }
        }

        @arr_dup_chars = sort { $chaifens->{$b->[0]}{weight} <=> $chaifens->{$a->[0]}{weight} || $a->[0] cmp $b->[0] } @arr_dup_chars;
        $arr_arr_dup_chars[$i] = \@arr_dup_chars;
    }

    return ( \@static_dups, \@dynamic_dups, \@arr_arr_dup_chars );
}

sub dump_dups($total_weights, $clusters, $static_dups, $dynamic_dups, $arr_arr_dup_chars) {
    my @roots = sort { $clusters->{$a} <=> $clusters->{$b} || $a cmp $b } keys %$clusters;

    print "聚类字根： ", scalar(@roots), " @roots\n";

    for my $i (0 .. 4) {
        printf "[%d] 动重: %.4f%%\n", $i, 100 * $dynamic_dups->[$i] / $total_weights;
        printf "[%d] 静重: %d\n", $i, $static_dups->[$i];

        my @a = @{ $arr_arr_dup_chars->[$i] };
        printf "[%d] 组数: %d\n", $i, scalar(@a);

        for (my $j = 0; $j < @a; ++$j) {
            my @b = @{ $a[$j] };

            # 每一码有 19 种取值([^aeuioz])，二、三、四码有 25 种取值，保守起见只看 9 重以上
            next if $i < 4 && @b <= $max_dups;

            printf "[%d]  %3d %s\n", $i, scalar(@b), join(" ", @b);
        }
    }

    print "\n";
}
