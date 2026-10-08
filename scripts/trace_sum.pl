#!/usr/bin/perl
# Sum a Bevy chrome trace (bevy/trace_chrome) by span name: total and per-frame ms.
# usage: perl trace_sum.pl trace.json [skip_seconds] [top_n]
use strict; use warnings;
my ($file, $skip, $top) = @ARGV; $skip //= 10; $top //= 60;
open my $fh, '<', $file or die "$file: $!";
my (%stack, %tot, %cnt, $t0, $frames, $first_frame_ts, $last_frame_ts);
$frames = 0;
while (my $l = <$fh>) {
  next unless $l =~ /"ph":"([BEX])"/; my $ph = $1;
  my ($ts) = $l =~ /"ts":([0-9.]+)/; next unless defined $ts;
  $t0 //= $ts;
  my ($tid) = $l =~ /"tid":(\d+)/; $tid //= 0;
  my ($name) = $l =~ /"name":"((?:[^"\\]|\\.)*)"/; $name //= '?';
  $name =~ s/\\"/"/g;
  if ($ph eq 'B') { push @{$stack{$tid}}, [$name, $ts]; next; }
  my ($n, $start, $dur);
  if ($ph eq 'X') { ($dur) = $l =~ /"dur":([0-9.]+)/; ($n, $start) = ($name, $ts); }
  else { my $e = pop @{$stack{$tid}} or next; ($n, $start) = @$e; $dur = $ts - $start; }
  next if $start - $t0 < $skip * 1e6;
  if ($n eq 'frame') { $frames++; $first_frame_ts //= $start; $last_frame_ts = $start + $dur; }
  $tot{$n} += $dur; $cnt{$n}++;
}
die "no frames after skip\n" unless $frames;
printf "frames %d, wall %.1f s, mean frame %.2f ms\n", $frames, ($last_frame_ts - $first_frame_ts) / 1e6, ($last_frame_ts - $first_frame_ts) / 1e3 / $frames;
my @k = sort { $tot{$b} <=> $tot{$a} } keys %tot;
for my $k (@k[0 .. ($top < @k ? $top - 1 : $#k)]) {
  printf "%9.3f ms/frame %8d calls  %s\n", $tot{$k} / 1e3 / $frames, $cnt{$k}, substr($k, 0, 140);
}
