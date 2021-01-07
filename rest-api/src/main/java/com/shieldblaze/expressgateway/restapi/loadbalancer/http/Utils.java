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
package com.shieldblaze.expressgateway.restapi.loadbalancer.http;

import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRandom;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.StickySession;
import com.shieldblaze.expressgateway.configuration.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.Transformer;
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;

import java.io.IOException;

final class Utils {

    static HTTPBalance determineAlgorithm(HTTPLoadBalancerContext httpLoadBalancerContext) {

        // Round-Robin
        if (httpLoadBalancerContext.algorithm().equalsIgnoreCase("HTTPRoundRobin")) {
            if (httpLoadBalancerContext.sessionPersistence().equalsIgnoreCase("StickySession")) {
                return new HTTPRoundRobin(new StickySession());
            } else if (httpLoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE);
            }
        }

        // Random
        if (httpLoadBalancerContext.algorithm().equalsIgnoreCase("HTTPRandom")) {
            if (httpLoadBalancerContext.sessionPersistence().equalsIgnoreCase("StickySession")) {
                return new HTTPRandom(new StickySession());
            } else if (httpLoadBalancerContext.sessionPersistence().equalsIgnoreCase("NOOP")) {
                return new HTTPRandom(NOOPSessionPersistence.INSTANCE);
            }
        }

        throw new IllegalArgumentException("Unknown Algorithm or SessionPersistence: " +
                httpLoadBalancerContext.algorithm() + ", " + httpLoadBalancerContext.sessionPersistence());
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
