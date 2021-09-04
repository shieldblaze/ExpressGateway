#!/bin/bash
#
# This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
# Copyright (c) 2020-2021 ShieldBlaze
#
# ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
#

#Set CPU Governor
cpupower frequency-set -g performance

# Max file and memory
ulimit -n 20000000
ulimit -l 20000000

# Turn off firewalld and set conntrack to notrack.
service firewalld stop
iptables -t raw -A PREROUTING -j NOTRACK

# -- System Tuning --
for iface in $(ifconfig | cut -d ' ' -f1| tr ':' '\n' | awk NF)
do
        ifconfig "$iface" txqueuelen 10000
        tc qdisc add dev "$iface" root fq
done

exit 0