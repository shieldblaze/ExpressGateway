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
package com.shieldblaze.expressgateway.restapi.loadbalancer.l4;

import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastConnection;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastLoad;
import com.shieldblaze.expressgateway.backend.strategy.l4.Random;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.FourTupleHash;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.SourceIPHash;
import com.shieldblaze.expressgateway.configuration.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.Transformer;
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;

import java.io.IOException;

final class Utils {

    static L4Balance determineAlgorithm(L4LoadBalancerContext l4LoadBalancerContext) {

        // Round-Robin
        if (l4LoadBalancerContext.algorithm().equalsIgnoreCase("RoundRobin")) {
            if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                return new RoundRobin(new SourceIPHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("FourTupleHash")) {
                return new RoundRobin(new FourTupleHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new RoundRobin(NOOPSessionPersistence.INSTANCE);
            }
        }

        // Random
        if (l4LoadBalancerContext.algorithm().equalsIgnoreCase("Random")) {
            if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                return new Random(new SourceIPHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("FourTupleHash")) {
                return new Random(new FourTupleHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new Random(NOOPSessionPersistence.INSTANCE);
            }
        }

        // Least Connection
        if (l4LoadBalancerContext.algorithm().equalsIgnoreCase("LeastConnection")) {
            if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                return new LeastConnection(new SourceIPHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("FourTupleHash")) {
                return new LeastConnection(new FourTupleHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new LeastConnection(NOOPSessionPersistence.INSTANCE);
            }
        }

        // Least Load
        if (l4LoadBalancerContext.algorithm().equalsIgnoreCase("LeastLoad")) {
            if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                return new LeastLoad(new SourceIPHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("FourTupleHash")) {
                return new LeastLoad(new FourTupleHash());
            } else if (l4LoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new LeastLoad(NOOPSessionPersistence.INSTANCE);
            }
        }

        throw new IllegalArgumentException("Unknown Algorithm or SessionPersistence: " +
                l4LoadBalancerContext.algorithm() + ", " + l4LoadBalancerContext.sessionPersistence());
    }

    static L4FrontListener determineListener(String protocol) {
        if (protocol.equalsIgnoreCase("tcp")) {
            return new TCPListener();
        } else if (protocol.equalsIgnoreCase("udp")) {
            return new UDPListener();
        }

        return null;
    }

    static CoreConfiguration coreConfiguration(String profile) throws IOException {
        return CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration((TransportConfiguration) Transformer.read(TransportConfiguration.EMPTY_INSTANCE, profile))
                .withEventLoopConfiguration((EventLoopConfiguration) Transformer.read(EventLoopConfiguration.EMPTY_INSTANCE, profile))
                .withBufferConfiguration((BufferConfiguration) Transformer.read(BufferConfiguration.EMPTY_INSTANCE, profile))
                .build();
    }

    static TLSServerConfiguration tlsForServer(String profile) throws IOException {
        return (TLSServerConfiguration) Transformer.read(TLSServerConfiguration.EMPTY_INSTANCE, profile);
    }

    static TLSClientConfiguration tlsForClient(String profile) throws IOException {
        return (TLSClientConfiguration) Transformer.read(TLSClientConfiguration.EMPTY_INSTANCE, profile);
    }
}
