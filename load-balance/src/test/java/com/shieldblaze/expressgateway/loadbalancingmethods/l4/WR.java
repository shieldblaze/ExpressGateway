package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.google.common.collect.Range;
import com.google.common.collect.RangeMap;
import com.google.common.collect.TreeRangeMap;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

public class WR {


    public static Map<String, Integer> ipMap = new ConcurrentHashMap<>();

    public static void main(String[] args) {
        ipMap.put("10.10.1.1", 25);
        ipMap.put("10.10.1.2", 25);
        ipMap.put("10.10.1.3", 25);
        ipMap.put("10.10.1.4", 25);


        Set<String> servers = ipMap.keySet();

        TreeRangeMap<Integer, String> treeRangeMap = TreeRangeMap.create();


        int totalWeight = 0;
        for (String server : servers) {
            Integer weight = ipMap.get(server);

            treeRangeMap.put(Range.closed(totalWeight,  totalWeight += weight), server);
        }

        System.out.println(treeRangeMap);
        System.out.println(treeRangeMap.asMapOfRanges().size());

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;
        int index = 0;

        for (int i = 0; i < 10000; i++) {

            if (index > totalWeight) {
                index = 0;
            }

            switch (treeRangeMap.get(index)) {
                case "10.10.1.1": {
                    first++;
                    break;
                }
                case "10.10.1.2": {
                    second++;
                    break;
                }
                case "10.10.1.3": {
                    third++;
                    break;
                }
                case "10.10.1.4": {
                    forth++;
                    break;
                }
                default:
                    break;
            }

            index++;
        }

        System.out.println(first);
        System.out.println(second);
        System.out.println(third);
        System.out.println(forth);
    }
}
