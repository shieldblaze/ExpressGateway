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
package com.shieldblaze.expressgateway.restapi.loadbalancer;

import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.FourTupleHash;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.SourceIPHash;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transformer.EventLoopTransformer;
import com.shieldblaze.expressgateway.configuration.transformer.PooledByteBufAllocatorTransformer;
import com.shieldblaze.expressgateway.configuration.transformer.TransportTransformer;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;

import java.io.IOException;

final class Utils {

    static L4Balance determineAlgorithm(L4HandlerContext l4HandlerContext) {

        if (l4HandlerContext.algorithm().equalsIgnoreCase("RoundRobin")) {
            if (l4HandlerContext.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                return new RoundRobin(new SourceIPHash());
            } else if (l4HandlerContext.sessionPersistence().equalsIgnoreCase("FourTupleHash")) {
                return new RoundRobin(new FourTupleHash());
            } else if (l4HandlerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new RoundRobin(NOOPSessionPersistence.INSTANCE);
            }
        }

        return null;
    }

    static L4FrontListener determineListener(L4HandlerContext l4HandlerContext) {
        if (l4HandlerContext.protocol().equalsIgnoreCase("tcp")) {
            return new TCPListener();
        } else if (l4HandlerContext.protocol().equalsIgnoreCase("udp")) {
            return new UDPListener();
        }

        return null;
    }

    static CoreConfiguration coreConfiguration() throws IOException {
        return CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(TransportTransformer.readFile())
                .withEventLoopConfiguration(EventLoopTransformer.readFile())
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorTransformer.readFile())
                .build();
    }
}
