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
package com.shieldblaze.expressgateway.integration.aws.ec2.route53;

import com.shieldblaze.expressgateway.common.utils.NumberUtil;

import java.util.Objects;

public final class Route53DNSRecordBody {
    private final String hostedZoneId;
    private final String fqdn;
    private final String type;
    private final String target;
    private final long ttl;

    public Route53DNSRecordBody(String hostedZoneId, String fqdn, String type, String target, long ttl) {
        this.hostedZoneId = Objects.requireNonNull(hostedZoneId, "HostedZoneID");
        this.fqdn = Objects.requireNonNull(fqdn, "FQDN");
        this.type = Objects.requireNonNull(type, "Type");
        this.target = Objects.requireNonNull(target, "Target");
        this.ttl = NumberUtil.checkPositive(ttl, "TTL");
    }

    public String hostedZoneId() {
        return hostedZoneId;
    }

    public String fqdn() {
        return fqdn;
    }

    public String type() {
        return type;
    }

    public String target() {
        return target;
    }

    public long ttl() {
        return ttl;
    }
}
