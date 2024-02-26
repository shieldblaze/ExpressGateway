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
package com.shieldblaze.expressgateway.servicediscovery.client;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.servicediscovery.server.ServiceDiscoveryServer;
import io.netty.handler.ssl.ClientAuth;
import org.apache.curator.test.TestingServer;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.File;
import java.io.IOException;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class ServiceDiscoveryClientTest {

    private static TestingServer zooKeeperServer;

    static {
        ClassLoader classLoader = ServiceDiscoveryClientTest.class.getClassLoader();
        File file = new File(classLoader.getResource("configuration.json").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty("config.file", absolutePath);
    }

    @BeforeAll
    static void setup() throws Exception {
        zooKeeperServer = new TestingServer(9001);
        ServiceDiscoveryServer.main(new String[0]);

        ExpressGateway expressGateway = new ExpressGateway(
                ExpressGateway.RunningMode.REPLICA,
                "1-2-3-4-5-f",
                Environment.TESTING,
                new ExpressGateway.RestApi("127.0.0.1", 9110, false, ClientAuth.NONE, "", ""),
                new ExpressGateway.ZooKeeper(),
                new ExpressGateway.ServiceDiscovery("https://localhost:45000", true, false, "",
                        "", "", ""),
                new ExpressGateway.LoadBalancerTLS()
        );

        ExpressGateway.setInstance(expressGateway);
    }

    @AfterAll
    static void shutdown() throws IOException {
        ServiceDiscoveryServer.shutdown();
        if (zooKeeperServer != null) {
            zooKeeperServer.close();
        }
    }

    @Order(1)
    @Test
    void register() throws Exception {
        ServiceDiscoveryClient.register();
    }

    @Order(2)
    @Test
    void deregister() throws Exception {
        ServiceDiscoveryClient.deregister();
    }
}
