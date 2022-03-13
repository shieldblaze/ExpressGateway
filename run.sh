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

MAVEN_OPTS="-Xms512m -Xmx1024m -XX:+UseShenandoahGC"
export MAVEN_OPTS

#Initialize everything before running
sudo ./initialize.sh

#Compile and install modules in local Maven repository
mvn clean install

#Start ExpressGateway
mvn exec:java -pl bootstrap -Dexec.mainClass="com.shieldblaze.expressgateway.bootstrap.ExpressGateway"
