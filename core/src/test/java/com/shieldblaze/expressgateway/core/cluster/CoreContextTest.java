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

package com.shieldblaze.expressgateway.core.cluster;

import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class CoreContextTest {

    @Test
    void simpleAddGetRemoveTest() {
        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new DummyL4FrontListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .build();

        L4FrontListenerStartupTask l4FrontListenerStartupEvent = l4LoadBalancer.start();

        CoreContext.add(l4LoadBalancer.id(), l4LoadBalancer);
        assertEquals(l4FrontListenerStartupEvent, CoreContext.getContext(l4LoadBalancer.id()).event());
        assertEquals(l4FrontListenerStartupEvent, CoreContext.remove(l4LoadBalancer.id()).event());
    }

    private static final class DummyL4FrontListener extends L4FrontListener {

        @Override
        public L4FrontListenerStartupTask start() {
            return new L4FrontListenerStartupTask();
        }

        @Override
        public L4FrontListenerStopTask stop() {
            return new L4FrontListenerStopTask();
        }

        @Override
        public L4FrontListenerShutdownTask shutdown() {
            return new L4FrontListenerShutdownTask();
        }
    }
}
