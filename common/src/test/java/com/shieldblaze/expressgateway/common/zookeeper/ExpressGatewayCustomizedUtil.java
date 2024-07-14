/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.common.zookeeper;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import io.netty.handler.ssl.ClientAuth;

import java.util.UUID;

final class ExpressGatewayCustomizedUtil {

    private ExpressGatewayCustomizedUtil() {
        // Prevent outside initialization
    }

    static ExpressGateway forTest(String zooKeeperConnectionString) {
        ExpressGateway.ZooKeeper zooKeeper = new ExpressGateway.ZooKeeper(zooKeeperConnectionString,
                3,
                100,
                false,
                false,
                "",
                "",
                "",
                "");

        return forTest(zooKeeper);
    }

    static ExpressGateway forTest(ExpressGateway.ZooKeeper zooKeeper) {
        ExpressGateway.RestApi restApi = new ExpressGateway.RestApi("127.0.0.1", 9110, false, ClientAuth.NONE, "", "");

        ExpressGateway.ServiceDiscovery serviceDiscovery = new ExpressGateway.ServiceDiscovery("http://127.0.0.1:25250/ExpressGateway/Servers",
                true, false, "", "", "", ""
        );

        ExpressGateway.LoadBalancerTLS loadBalancerTLS = new ExpressGateway.LoadBalancerTLS(false,
                "",
                "");

        return new ExpressGateway(ExpressGateway.RunningMode.REPLICA,
                UUID.randomUUID().toString(),
                Environment.DEVELOPMENT,
                restApi,
                zooKeeper,
                serviceDiscovery,
                loadBalancerTLS);
    }
}
