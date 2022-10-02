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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import io.netty.channel.ChannelFuture;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NodeTest {

    @Test
    void maxConnectionTest() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE)).build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9110))
                .build();

        assertThrows(IllegalArgumentException.class, () -> node.maxConnections(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> node.maxConnections(-2));
        assertDoesNotThrow(() -> node.maxConnections(1));
        assertDoesNotThrow(() -> node.maxConnections(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> node.maxConnections(5000));

        // Add 5000 fake connections
        for (int i = 0; i < 5000; i++) {
            node.incActiveConnection0();
        }

        // Add 1 more connection and it should throw TooManyConnectionsException.
        assertThrows(TooManyConnectionsException.class, () -> node.addConnection(new DummyConnection(node)));

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    private static final class DummyConnection extends Connection {

        private DummyConnection(Node node) {
            super(node);
        }

        @Override
        protected void processBacklog(ChannelFuture channelFuture) {
            // Does nothing
        }
    }
}
