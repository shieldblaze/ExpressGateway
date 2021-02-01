/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfigurationBuilder;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class NodeBytesTrackerTest {

    @Test
    void receive10MBytes() {
        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE));
        Node node = new Node(cluster, new InetSocketAddress("127.0.0.1", 9110));

        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new NodeBytesTracker(node));

        for (int i = 1; i <= 10_000_000; i++) {
            ByteBuf byteBuf = Unpooled.buffer().writeZero(1);
            embeddedChannel.writeInbound(byteBuf);
            embeddedChannel.flushInbound();
            byteBuf.release();

            assertEquals(i, node.bytesReceived());
        }

        assertEquals(0, node.bytesSent());

        embeddedChannel.close();
    }

    @Test
    void send10MBytes() {
        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE));
        Node node = new Node(cluster, new InetSocketAddress("127.0.0.1", 9110));

        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new NodeBytesTracker(node));

        for (int i = 1; i <= 10_000_000; i++) {
            ByteBuf byteBuf = Unpooled.buffer().writeZero(1);
            embeddedChannel.writeOutbound(byteBuf);
            embeddedChannel.flushOutbound();
            byteBuf.release();

            assertEquals(i, node.bytesSent());
        }

        assertEquals(0, node.bytesReceived());

        embeddedChannel.close();
    }
}
