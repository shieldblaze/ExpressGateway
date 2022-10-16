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
package com.shieldblaze.expressgateway.common.curator;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import org.apache.curator.test.TestingServer;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.UUID;

import static com.shieldblaze.expressgateway.common.curator.Curator.getInstance;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.createNew;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.deleteData;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.doesPathExists;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.getData;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.setData;
import static com.shieldblaze.expressgateway.common.curator.ExpressGatewayUtils.forTest;
import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CuratorUtilsTest {

    private static TestingServer testingServer;

    @BeforeAll
    static void setUp() throws Exception {
        testingServer = new TestingServer();
        testingServer.start();

        ExpressGateway.setInstance(forTest(testingServer.getConnectString()));
        Curator.init();
    }

    @AfterAll
    static void shutdown() throws Exception {
        try {
            deleteData(getInstance(), ZNodePath.create("expressgateway"), true);
        } finally {
            Curator.shutdown();
            testingServer.close();
        }
    }

    @Test
    void allCaseCombinedTest() throws Exception {
        ZNodePath zNodePath = ZNodePath.create("expressgateway", Environment.detectEnv(), ExpressGateway.getInstance().clusterID(), "utils");

        // Path does not exist yet
        assertFalse(doesPathExists(getInstance(), zNodePath));

        // Create a path and set data
        assertTrue(createNew(getInstance(), zNodePath, "ShieldBlaze".getBytes()));

        // Path does not exist yet
        assertTrue(doesPathExists(getInstance(), zNodePath));

        // Get the data
        assertEquals("ShieldBlaze", new String(getData(getInstance(), zNodePath)));

        // Set new data
        assertTrue(setData(getInstance(), zNodePath, "ExpressGateway".getBytes()));

        // Get the data
        assertEquals("ExpressGateway", new String(getData(getInstance(), zNodePath)));

        // Delete the ZNode
        assertDoesNotThrow(() -> deleteData(getInstance(), zNodePath, true));

        // Path is now deleted
        assertFalse(doesPathExists(getInstance(), zNodePath));
    }
}
