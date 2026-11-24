scoreboard objectives add dpp.version dummy
scoreboard players set #protocol dpp.version 1
data merge storage dpp:meta {version:"0.1.0",protocol:"dpp.events/1"}
