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
package com.shieldblaze.expressgateway.controlinterface.loadbalancer;

import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastConnection;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastLoad;
import com.shieldblaze.expressgateway.backend.strategy.l4.Random;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.FourTupleHash;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.SourceIPHash;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRandom;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.StickySession;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;

import java.util.concurrent.atomic.AtomicBoolean;

@SuppressWarnings({"unchecked", "rawtypes"})
public final class Common {

    public static SessionPersistence l4(String sessionPersistence) {
        if (sessionPersistence.equalsIgnoreCase("FourTupleHash")) {
            return new FourTupleHash();
        } else if (sessionPersistence.equalsIgnoreCase("SourceIPHash")) {
            return new SourceIPHash();
        }

        return NOOPSessionPersistence.INSTANCE;
    }

    public static LoadBalance l4(String strategy, SessionPersistence sessionPersistence) {
        if (strategy.equalsIgnoreCase("LeastConnection")) {
            return new LeastConnection(sessionPersistence);
        } else if (strategy.equalsIgnoreCase("LeastLoad")) {
            return new LeastLoad(sessionPersistence);
        } else if (strategy.equalsIgnoreCase("Random")) {
            return new Random(sessionPersistence);
        }

        return new RoundRobin(sessionPersistence);
    }

    public static SessionPersistence l7(String sessionPersistence) {
        if (sessionPersistence.equalsIgnoreCase("StickySession")) {
            return new StickySession();
        }

        return com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence.INSTANCE;
    }

    public static LoadBalance l7(String strategy, SessionPersistence sessionPersistence) {
        if (strategy.equalsIgnoreCase("HTTPRandom")) {
            return new HTTPRandom(sessionPersistence);
        }

        return new HTTPRoundRobin(sessionPersistence);
    }
}
