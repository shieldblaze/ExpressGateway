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
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import org.apache.curator.framework.recipes.cache.ChildData;
import org.apache.curator.test.TestingServer;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.util.List;
import java.util.Optional;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.zookeeper.CertificateManager.retrieveEntry;
import static com.shieldblaze.expressgateway.common.zookeeper.CertificateManager.storeEntry;
import static com.shieldblaze.expressgateway.common.zookeeper.ExpressGatewayCustomizedUtil.forTest;
import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class CertificateManagerTest {

    private static final byte[] data = new byte[1024];
    private static final String HOSTNAME = "expressgateway.shieldblaze.com";
    private static final SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of(HOSTNAME));
    private static TestingServer testingServer;

    @BeforeAll
    static void setUp() throws Exception {
        testingServer = new TestingServer();
        testingServer.start();

        ExpressGateway.setInstance(forTest(testingServer.getConnectString()));
        Curator.init();
        CertificateManager.INSTANCE.isInitialized().get(30, TimeUnit.SECONDS);
    }

    @AfterAll
    static void shutdown() throws Exception {
        try {
            CuratorUtils.deleteData(Curator.getInstance(), ZNodePath.create("ExpressGateway", Environment.detectEnv()), true);
        } finally {
            Curator.shutdown();
            testingServer.close();
        }
    }

    @Order(1)
    @Test
    void storeEntryTest() throws Exception {
        CryptoEntry cryptoEntry = new CryptoEntry(ssc.keyPair().getPrivate(), ssc.x509Certificate());
        storeEntry(true, HOSTNAME, cryptoEntry);

        Thread.sleep(1000); // 1 second should be enough for sync
    }

    @Order(2)
    @Test
    void retrieveEntryTest() throws Exception {
        CryptoEntry cryptoEntry = retrieveEntry(true, HOSTNAME);
        assertNotNull(cryptoEntry);
        assertArrayEquals(ssc.keyPair().getPrivate().getEncoded(), cryptoEntry.privateKey().getEncoded());
        assertArrayEquals(ssc.x509Certificate().getEncoded(), cryptoEntry.certificates()[0].getEncoded());
    }

    @Order(3)
    @Test
    void storeInZooKeeperTest() throws Exception {
        CertificateManager.store(true, "test." + HOSTNAME, data);

        Thread.sleep(1000); // 1 second should be enough for sync
    }

    @Order(4)
    @Test
    void retrieveTest() {
        Optional<ChildData> childData = CertificateManager.retrieve(true, "test." + HOSTNAME);
        assertTrue(childData.isPresent());
        assertArrayEquals(data, childData.get().getData());
    }

    @Order(5)
    @Test
    void removeFromZooKeeperTest() throws Exception {
        CertificateManager.remove(true, "test." + HOSTNAME);
    }
}
