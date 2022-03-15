#!/bin/bash
#
# This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
# Copyright (c) 2020-2022 ShieldBlaze
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

# -- Copy TCNative binary file to /usr/lib so we can load it easily --
sudo cp bin/libnetty_tcnative.so /usr/lib/


# -- Sysctl Tuning --

sudo echo "
net.core.rmem_max=2147483647
net.core.wmem_max=2147483647
net.ipv4.tcp_rmem="4096 87380 2147483647"
net.ipv4.tcp_wmem="4096 65536 2147483647"
net.ipv4.udp_mem="4096 65536 2147483647"
net.ipv4.ip_local_port_range="2000 65535"
net.ipv4.tcp_max_syn_backlog=524288
net.ipv4.tcp_syn_retries=1
net.ipv4.tcp_synack_retries=1
net.core.optmem_max=4194304
net.ipv4.tcp_low_latency=1
net.ipv4.tcp_adv_win_scale=1
net.ipv4.tcp_rfc1337=1
net.ipv4.tcp_timestamps=1
net.ipv4.tcp_sack=1
net.ipv4.tcp_syncookies=1
net.ipv4.tcp_tw_reuse=1
net.core.netdev_max_backlog=524288
fs.file-max=52428800
fs.nr_open=52428800
" | sudo tee /etc/sysctl.conf

 sudo sysctl -p

# -- Upgrade Kernel to latest mainline kernel, disable SELinux and Reboot the Server --
if [ -f /etc/redhat-release ]; then
  sudo rpm --import https://yum.corretto.aws/corretto.key
  sudo curl -L -o /etc/yum.repos.d/corretto.repo https://yum.corretto.aws/corretto.repo
  sudo yum update -y
  sudo yum install epel-release tc net-tools java-11-amazon-corretto-devel -y
  sudo rpm --import https://www.elrepo.org/RPM-GPG-KEY-elrepo.org
  sudo yum install https://www.elrepo.org/elrepo-release-8.el8.elrepo.noarch.rpm -y
  sudo yum update -y
  sudo yum --enablerepo=elrepo-kernel install kernel-ml -y

# -- Disable SELinux --
  sudo echo "
SELINUX=disabled
SELINUXTYPE=targeted
" | sudo tee /etc/sysconfig/selinux

#Reboot the server
  sudo reboot
fi