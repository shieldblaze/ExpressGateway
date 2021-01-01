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
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transformer.EventLoopTransformer;
import com.shieldblaze.expressgateway.configuration.transformer.PooledByteBufAllocatorTransformer;
import com.shieldblaze.expressgateway.configuration.transformer.TLSTransformer;
import com.shieldblaze.expressgateway.configuration.transformer.TransportTransformer;

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

    static CoreConfiguration coreConfiguration() throws IOException {
        return CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(TransportTransformer.readFile())
                .withEventLoopConfiguration(EventLoopTransformer.readFile())
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorTransformer.readFile())
                .build();
    }


    static TLSConfiguration tlsForServer() throws IOException {
        return TLSTransformer.readFile(true);
    }

    static TLSConfiguration tlsForClient() throws IOException {
        return TLSTransformer.readFile(false);
    }
}
