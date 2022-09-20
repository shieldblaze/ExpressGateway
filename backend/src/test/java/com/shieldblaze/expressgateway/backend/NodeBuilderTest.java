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
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NodeBuilderTest {

    @Test
    void simpleNodeBuilderTest() {
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE)).build();

        assertThrows(NullPointerException.class, () -> NodeBuilder.newBuilder().build());
        assertThrows(NullPointerException.class, () -> NodeBuilder.newBuilder().withSocketAddress(null).build());
        assertThrows(NullPointerException.class, () -> NodeBuilder.newBuilder().withCluster(null).build());
        assertThrows(NullPointerException.class, () -> NodeBuilder.newBuilder().withSocketAddress(new InetSocketAddress(9110)).build());
        assertDoesNotThrow(() -> NodeBuilder.newBuilder().withSocketAddress(new InetSocketAddress(9110)).withCluster(cluster).build());
    }
}
