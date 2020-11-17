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

package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.SingleBackendCluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertThrows;

class L4BalanceTest {

    @Test
    void testL4Balance() throws LoadBalanceException {
        Node node = new Node(new InetSocketAddress("192.168.1.1", 9110));

        L4Balance l4Balance = new EmptyL4Balance();
        l4Balance.cluster(SingleBackendCluster.of("localhost.domain", node));

        Assertions.assertEquals(node, l4Balance.response(null).backend());
    }

    @Test
    void throwsException() {
        assertThrows(NullPointerException.class, () -> new EmptyL4Balance().cluster(null));
    }

    private static final class EmptyL4Balance extends L4Balance {

        public EmptyL4Balance() {
            super(new NOOPSessionPersistence());
        }

        @Override
        public L4Response response(L4Request l4Request) {
            return new L4Response(cluster.get(0));
        }
    }
}
