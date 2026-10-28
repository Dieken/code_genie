#!/usr/bin/env perl
# 例：./generate-lingyun-words-dict.pl --codes lingyun-full-codes.tsv --words words.txt
# 单字全码表：字<TAB>全码；词库：词[<TAB>频率]。不指定 --words 时读取标准输入。

use v5.36;
use utf8;
use warnings qw(FATAL utf8);
use open qw(:std :encoding(UTF-8));
use Encode qw(decode);
use Getopt::Long;
use autodie;

@ARGV = map { decode('UTF-8', $_, Encode::FB_CROAK) } @ARGV;
my ($codes_file, $words_file, $help);
GetOptions('codes=s' => \$codes_file, 'words=s' => \$words_file, 'help' => \$help)
    or die "参数错误；使用 --help 查看用法。\n";
if ($help) {
    print "用法：$0 --codes 单字全码表.tsv [--words 词库.txt]\n";
    exit 0;
}
die "必须提供 --codes 单字全码表.tsv，不能使用含简码的合并码表。\n" unless $codes_file;
die "未知参数：@ARGV\n" if @ARGV;

my %codes;
{
    open my $fh, '<', $codes_file;
    while (<$fh>) {
        next if /^\s*(?:#|$)/;
        chomp;
        my ($char, $code) = split;
        die "全码表格式错误，第 $. 行：$_\n"
            unless defined $code && $char =~ /^\p{Han}$/ && $code =~ /^[a-zA-Z]{3,4}$/;
        $code = lc $code;
        die "字符 $char 有多个不同全码，请只提供单字全码表。\n"
            if exists $codes{$char} && $codes{$char} ne $code;
        $codes{$char} = $code;
    }
}
die "单字全码表为空。\n" unless %codes;

my $words_fh = \*STDIN;
open $words_fh, '<', $words_file if defined $words_file;
while (<$words_fh>) {
    next if /^\s*(?:#|$)/;
    chomp;
    my ($word, $freq) = split;
    die "词库格式错误：$_；需为汉字词和可选数字频率。\n"
        unless $word =~ /^\p{Han}{2,}$/ && (!defined $freq || $freq =~ /^\d+(?:\.\d+)?$/);
    my @chars = split //, $word;
    my @missing = grep { !exists $codes{$_} } @chars;
    die "词「$word」缺少单字全码：@missing\n" if @missing;
    my @c = map { $codes{$_} } @chars;
    my $code;
    if (@c == 2) {
        $code = substr($c[0], 0, 2) . substr($c[1], 0, 2);
    } elsif (@c == 3) {
        $code = substr($c[0], 0, 1) . substr($c[1], 0, 1) . substr($c[2], 0, 2);
    } else {
        $code = join '', map { substr($_, 0, 1) } @c[0, 1, 2, -1];
    }
    print "$word\t$code", defined $freq ? "\t$freq" : '', "\n";
}
