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
package com.shieldblaze.expressgateway.testing;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;

import java.io.File;
import java.util.UUID;

/**
 * Generate a {@link ExpressGateway} instance configured specially for testing purpose.
 */
public final class ExpressGatewayConfigured {

    public static ExpressGateway forTest() {
        return forTest("localhost:2181");
    }

    public static ExpressGateway forTest(String zooKeeperConnectionString) {
        ClassLoader classLoader = ExpressGatewayConfigured.class.getClassLoader();
        File file = new File(classLoader.getResource("default").getFile());
        String absolutePath = file.getAbsolutePath();

        ExpressGateway.RestApi restApi = new ExpressGateway.RestApi("127.0.0.1",
                9110,
                true,
                absolutePath + File.separator + "restapi.p12", "shieldblaze");

        ExpressGateway.ZooKeeper zooKeeper = new ExpressGateway.ZooKeeper(zooKeeperConnectionString,
                3,
                100,
                false,
                false,
                "",
                "",
                "" ,
                "");

        ExpressGateway.LoadBalancerTLS loadBalancerTLS = new ExpressGateway.LoadBalancerTLS(false,
                "",
                "");

        return new ExpressGateway(ExpressGateway.RunningMode.STANDALONE,
                UUID.randomUUID().toString(),
                Environment.DEVELOPMENT,
                restApi,
                zooKeeper,
                loadBalancerTLS);
    }

    private ExpressGatewayConfigured() {
        // Prevent outside initialization
    }
}
