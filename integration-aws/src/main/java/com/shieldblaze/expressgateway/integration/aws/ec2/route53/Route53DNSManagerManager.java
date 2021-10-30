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

import com.shieldblaze.expressgateway.integration.dns.DNSManager;
import com.shieldblaze.expressgateway.integration.dns.DNSRecord;
import com.shieldblaze.expressgateway.integration.aws.AWS;
import com.shieldblaze.expressgateway.integration.event.DNSAddedEvent;
import com.shieldblaze.expressgateway.integration.event.DNSRemovedEvent;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.route53.Route53Client;
import software.amazon.awssdk.services.route53.model.ListResourceRecordSetsRequest;
import software.amazon.awssdk.services.route53.model.ListResourceRecordSetsResponse;
import software.amazon.awssdk.services.route53.model.RRType;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public final class Route53DNSManagerManager extends AWS implements DNSManager<Route53DNSRecordBody>, Runnable, Closeable {

    private final ScheduledExecutorService executorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> scheduledFuture;

    private final List<DNSRecord> dnsRecordList = new CopyOnWriteArrayList<>();
    private final Route53Client route53Client;
    private final Route53DNS route53DNS;
    private final String hostedZoneId;

    public Route53DNSManagerManager(AwsCredentialsProvider awsCredentialsProvider, String hostedZoneId) {
        super(awsCredentialsProvider, Region.AWS_GLOBAL);
        this.hostedZoneId = Objects.requireNonNull(hostedZoneId, "HostedZoneID");

        route53Client = Route53Client.builder()
                .credentialsProvider(awsCredentialsProvider)
                .region(Region.AWS_GLOBAL)
                .build();

        route53DNS = new Route53DNS(route53Client);
        scheduledFuture = executorService.scheduleAtFixedRate(this, 0, 30, TimeUnit.SECONDS);
    }

    @Override
    public void run() {
        List<DNSRecord> dnsList = new ArrayList<>();

        ListResourceRecordSetsResponse resourceRecordSetsResponse = route53Client.listResourceRecordSets(ListResourceRecordSetsRequest.builder()
                .hostedZoneId(hostedZoneId)
                .maxItems(String.valueOf(Integer.MAX_VALUE))
                .build());

        resourceRecordSetsResponse.resourceRecordSets()
                .stream()
                .filter(resourceRecordSet -> (resourceRecordSet.type() == RRType.A || resourceRecordSet.type() == RRType.AAAA) &&
                        !resourceRecordSet.resourceRecords().isEmpty())
                .forEach(resourceRecordSet -> dnsList.add(Route53DNSRecord.buildFrom(resourceRecordSet)));

        dnsRecordList.clear();
        dnsRecordList.addAll(dnsList);
    }

    @Override
    public List<DNSRecord> dnsRecords() {
        return dnsRecordList;
    }

    @Override
    public DNSAddedEvent<Boolean> add(Route53DNSRecordBody add) {
        return route53DNS.addRecord(add);
    }

    @Override
    public DNSRemovedEvent<Boolean> remove(Route53DNSRecordBody remove) {
        return route53DNS.removeRecord(remove);
    }

    @Override
    public void close() {
        scheduledFuture.cancel(true);
        executorService.shutdown();
        route53Client.close();
    }
}
