/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ClusterPoolTest {

    private final EventStream eventStream = new EventStream();

    @Test
    void testStaticNaming() {
        for (int i = 0; i < 100_000; i++) {
            Cluster cluster = new ClusterPool(eventStream, new RoundRobin(NOOPSessionPersistence.INSTANCE));
            assertEquals("ClusterPool#" + i, cluster.name());
        }
    }

    @Test
    void testNodeAdd() {
        Cluster cluster = new ClusterPool(eventStream, new RoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 1));

        for (int i = 0; i < 100_000; i++) {
            assertFalse(cluster.addNode(new Node(cluster, new InetSocketAddress("127.0.0.1", 1))));
        }
    }
}
