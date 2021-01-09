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
package com.shieldblaze.expressgateway.security;

import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;
import io.github.bucket4j.Bucket4j;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.ipfilter.IpSubnetFilter;
import io.netty.handler.ipfilter.IpSubnetFilterRule;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Collections;
import java.util.List;

/**
 * {@link NACL} handles Access Control List (ACL) for allowing/denying IP Addresses
 * and Rate-Limit every new connection.
 */
@ChannelHandler.Sharable
public final class NACL extends IpSubnetFilter {

    private static final Logger logger = LogManager.getLogger(NACL.class);

    private final Bucket bucket;

    /**
     * Create a new {@link NACL} Instance with Rate-Limit Disabled
     *
     * @param ipSubnetFilterRules {@link List} of {@link IpSubnetFilterRule}
     * @param acceptIfNotFound    Set to {@code true} if we will accept connection if it's not found
     *                            in {@code ipSubnetFilterRules} else set to {@code false.}
     */
    public NACL(List<IpSubnetFilterRule> ipSubnetFilterRules, boolean acceptIfNotFound) {
        this(0, null, ipSubnetFilterRules, acceptIfNotFound);
    }

    /**
     * Create a new {@link NACL} Instance with ACL Disabled
     *
     * @param connections Number of connections for Rate-Limit
     * @param duration    {@link Duration} of Rate-Limit
     */
    public NACL(int connections, Duration duration) {
        this(connections, duration, Collections.emptyList(), true);
    }

    /**
     * Create a new {@link NACL} Instance
     *
     * @param connections         Number of connections for Rate-Limit
     * @param duration            {@link Duration} of Rate-Limit
     * @param ipSubnetFilterRules {@link List} of {@link IpSubnetFilterRule}
     * @param acceptIfNotFound    Set to {@code true} if we will accept connection if it's not found
     *                            in {@code ipSubnetFilterRules} else set to {@code false.}
     */
    public NACL(int connections, Duration duration, List<IpSubnetFilterRule> ipSubnetFilterRules, boolean acceptIfNotFound) {
        super(acceptIfNotFound, ipSubnetFilterRules);
        if (connections == 0 && duration == null) {
            bucket = null;
            logger.info("Connection Rate-Limit is Disabled");
        } else {
            Bandwidth limit = Bandwidth.simple(connections, duration);
            bucket = Bucket4j.builder().addLimit(limit).withNanosecondPrecision().build();
            logger.info("Connection Rate-Limit is Enabled for {} connection(s) in {}", connections, duration);
        }
    }

    @Override
    protected boolean accept(ChannelHandlerContext ctx, InetSocketAddress remoteAddress) {
        try {
            // If Bucket is not `null`, it means Rate-Limit is enabled.
            if (bucket != null) {
                if (!bucket.asAsync().tryConsume(1).get()) {
                    logger.debug("Rate-Limit exceeded, Denying new connection from {}", remoteAddress);
                    return false;
                }
            }
            return super.accept(ctx, remoteAddress);
        } catch (Exception ex) {
            // Ignore
        }
        return false;
    }
}
