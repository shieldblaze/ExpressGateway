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

import com.shieldblaze.expressgateway.common.annotation.Async;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.integration.aws.ec2.route53.events.Route53DNSAddedEvent;
import com.shieldblaze.expressgateway.integration.aws.ec2.route53.events.Route53DNSRemovedEvent;
import com.shieldblaze.expressgateway.integration.dns.DNSAddRecord;
import com.shieldblaze.expressgateway.integration.dns.DNSRemoveRecord;
import software.amazon.awssdk.services.route53.Route53Client;
import software.amazon.awssdk.services.route53.model.Change;
import software.amazon.awssdk.services.route53.model.ChangeAction;
import software.amazon.awssdk.services.route53.model.ChangeBatch;
import software.amazon.awssdk.services.route53.model.ChangeResourceRecordSetsRequest;
import software.amazon.awssdk.services.route53.model.ChangeResourceRecordSetsResponse;
import software.amazon.awssdk.services.route53.model.ChangeStatus;
import software.amazon.awssdk.services.route53.model.RRType;
import software.amazon.awssdk.services.route53.model.ResourceRecord;
import software.amazon.awssdk.services.route53.model.ResourceRecordSet;

public final class Route53DNS implements DNSAddRecord<Route53DNSRecordBody, Route53DNSAddedEvent>, DNSRemoveRecord<Route53DNSRecordBody, Route53DNSRemovedEvent> {

    private final Route53Client route53Client;

    Route53DNS(Route53Client route53Client) {
        this.route53Client = route53Client;
    }

    @Async
    @Override
    public Route53DNSAddedEvent addRecord(Route53DNSRecordBody route53DNSRecordBody) {
        Route53DNSAddedEvent event = new Route53DNSAddedEvent();

        GlobalExecutors.submitTask(() -> {
            try {
                Change change = Change.builder()
                        .action(ChangeAction.UPSERT)
                        .resourceRecordSet(ResourceRecordSet.builder()
                                .name(route53DNSRecordBody.fqdn())
                                .type(route53DNSRecordBody.type().equalsIgnoreCase("A") ? RRType.A : RRType.AAAA)
                                .resourceRecords(ResourceRecord.builder().value(route53DNSRecordBody.target()).build())
                                .ttl(route53DNSRecordBody.ttl())
                                .build())
                        .build();

                ChangeResourceRecordSetsResponse changeResourceRecordSetsResponse = route53Client.changeResourceRecordSets(ChangeResourceRecordSetsRequest.builder()
                        .changeBatch(ChangeBatch.builder().changes(change).build())
                        .hostedZoneId(route53DNSRecordBody.hostedZoneId())
                        .build());

                ChangeStatus changeStatus = changeResourceRecordSetsResponse.changeInfo().status();
                if (changeStatus == ChangeStatus.PENDING) {
                    event.markSuccess(true);
                } else {
                    event.markFailure(new IllegalArgumentException("ChangeInfo Status expected to be PENDING but got: " + changeStatus));
                }
            } catch (Exception ex) {
                event.markFailure(ex);
            }
        });

        return event;
    }

    @Async
    @Override
    public Route53DNSRemovedEvent removeRecord(Route53DNSRecordBody route53DNSRecordBody) {
        Route53DNSRemovedEvent event = new Route53DNSRemovedEvent();

        GlobalExecutors.submitTask(() -> {
            try {
                Change change = Change.builder()
                        .action(ChangeAction.DELETE)
                        .resourceRecordSet(ResourceRecordSet.builder()
                                .name(route53DNSRecordBody.fqdn())
                                .type(route53DNSRecordBody.type().equalsIgnoreCase("A") ? RRType.A : RRType.AAAA)
                                .resourceRecords(ResourceRecord.builder().value(route53DNSRecordBody.target()).build())
                                .ttl(route53DNSRecordBody.ttl())
                                .build())
                        .build();

                ChangeResourceRecordSetsResponse changeResourceRecordSetsResponse = route53Client.changeResourceRecordSets(ChangeResourceRecordSetsRequest.builder()
                        .changeBatch(ChangeBatch.builder().changes(change).build())
                        .hostedZoneId(route53DNSRecordBody.hostedZoneId())
                        .build());

                ChangeStatus changeStatus = changeResourceRecordSetsResponse.changeInfo().status();
                if (changeStatus == ChangeStatus.PENDING) {
                    event.markSuccess(true);
                } else {
                    event.markFailure(new IllegalArgumentException("ChangeInfo Status expected to be PENDING but got: " + changeStatus));
                }
            } catch (Exception ex) {
                event.markFailure(ex);
            }
        });

        return event;
    }
}
