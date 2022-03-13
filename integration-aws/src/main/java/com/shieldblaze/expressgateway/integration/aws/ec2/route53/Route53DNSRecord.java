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
package com.shieldblaze.expressgateway.integration.aws.ec2.route53;

import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.integration.dns.DNSRecord;
import software.amazon.awssdk.services.route53.model.RRType;
import software.amazon.awssdk.services.route53.model.ResourceRecordSet;

import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.Objects;

public final class Route53DNSRecord implements DNSRecord {

    private final String fqdn;
    private final InetAddress target;
    private final RecordType recordType;
    private final long ttl;

    public static Route53DNSRecord of(String fqdn, InetAddress target, RecordType recordType, long ttl) {
        return new Route53DNSRecord(fqdn, target, recordType, ttl);
    }

    private Route53DNSRecord(String fqdn, InetAddress target, RecordType recordType, long ttl) {
        this.fqdn = Objects.requireNonNull(fqdn, "FQDN");
        this.target = Objects.requireNonNull(target, "Target");
        this.recordType = Objects.requireNonNull(recordType, "RecordType");
        this.ttl = NumberUtil.checkPositive(ttl, "TTL");
    }

    @Override
    public String fqdn() {
        return fqdn;
    }

    @Override
    public InetAddress target() {
        return target;
    }

    @Override
    public RecordType type() {
        return recordType;
    }

    @Override
    public long ttl() {
        return ttl;
    }

    @Override
    public String providerName() {
        return "AWS-ROUTE53";
    }

    @Override
    public String toString() {
        return "Route53DNSRecord{" +
                "fqdn='" + fqdn + '\'' +
                ", target=" + target +
                ", recordType=" + recordType +
                ", ttl=" + ttl +
                '}';
    }

    public static Route53DNSRecord buildFrom(ResourceRecordSet resourceRecordSet) {
        RecordType recordType;
        if (resourceRecordSet.type() == RRType.A) {
            recordType = RecordType.A;
        } else if (resourceRecordSet.type() == RRType.AAAA) {
            recordType = RecordType.AAAA;
        } else {
            throw new IllegalArgumentException("Unsupported Record Type: " + resourceRecordSet.type());
        }

        try {
            return of(recordType.name(), InetAddress.getByName(resourceRecordSet.resourceRecords().get(0).value()), recordType, resourceRecordSet.ttl());
        } catch (UnknownHostException e) {
            // This can never happen
            return null;
        }
    }
}
