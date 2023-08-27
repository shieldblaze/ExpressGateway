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
import org.apache.curator.test.InstanceSpec;
import org.apache.curator.test.TestingServer;
import org.jetbrains.annotations.NotNull;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.File;
import java.util.HashMap;
import java.util.Map;

import static com.shieldblaze.expressgateway.common.zookeeper.ExpressGatewayCustomizedUtil.forTest;
import static org.apache.zookeeper.client.ZKClientConfig.SECURE_CLIENT;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class CuratorTest {

    @Order(1)
    @Test
    void connectToZooKeeperWithoutTLSTest() throws Exception {
        System.setProperty("zookeeper.serverCnxnFactory", "org.apache.zookeeper.server.NettyServerCnxnFactory");
        try (TestingServer testingServer = new TestingServer()) {

            ExpressGateway.setInstance(forTest(testingServer.getConnectString()));
            Curator.init();

            assertTrue(Curator.isInitialized().get());
        } finally {
            Curator.shutdown();
        }
    }

    @Order(2)
    @Test
    void connectToZooKeeperUsingTLSTest() throws Exception {
        ClassLoader classLoader = CuratorTest.class.getClassLoader();
        File file = new File(classLoader.getResource("default").getFile());
        String absolutePath = file.getAbsolutePath();

        int securePort = InstanceSpec.getRandomPort();
        InstanceSpec instanceSpec = instanceSpec(securePort, absolutePath);

        try (TestingServer testingServer = new TestingServer(instanceSpec, true)) {
            ExpressGateway.setInstance(forTest(new ExpressGateway.ZooKeeper("127.0.0.1:" + securePort,
                    3,
                    100,
                    true,
                    false,
                    "",
                    "",
                    absolutePath + File.separator + "TrustStore.jks",
                    "123456")));

            Curator.init();
            assertTrue(Curator.isInitialized().get());
        } finally {
            System.clearProperty(SECURE_CLIENT);
            System.clearProperty("zookeeper.ssl.keyStore.location");
            System.clearProperty("zookeeper.ssl.keyStore.password");
            System.clearProperty("zookeeper.ssl.hostnameVerification");
            System.clearProperty("zookeeper.ssl.trustStore.location");
            System.clearProperty("zookeeper.ssl.trustStore.password");
        }
    }

    @NotNull
    private static InstanceSpec instanceSpec(int securePort, String absolutePath) {
        Map<String, Object> customProperties = new HashMap<>();
        customProperties.put("secureClientPort", String.valueOf(securePort));
        customProperties.put("ssl.keyStore.location", absolutePath + File.separator + "KeyStore.jks");
        customProperties.put("ssl.keyStore.password", "123456");
        customProperties.put("ssl.trustStore.location", absolutePath + File.separator + "TrustStore.jks");
        customProperties.put("ssl.trustStore.password", "123456");
        customProperties.put("ssl.hostnameVerification", "false");
        customProperties.put("serverCnxnFactory", "org.apache.zookeeper.server.NettyServerCnxnFactory");

        return new InstanceSpec(null,
                InstanceSpec.getRandomPort(),
                -1,
                -1,
                true,
                -1,
                -1,
                -1,
                customProperties);
    }
}
