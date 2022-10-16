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

import java.util.concurrent.ExecutionException;

import static com.shieldblaze.expressgateway.common.curator.ExpressGatewayUtils.forTest;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CuratorTest {

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
        Curator.shutdown();
        testingServer.close();
    }

    @Test
    void successfulConnectionTest() throws ExecutionException, InterruptedException {
        assertTrue(Curator.connectionFuture().get());
    }
}
