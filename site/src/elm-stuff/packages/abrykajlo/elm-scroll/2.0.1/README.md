# Elm Scroll

An Elm Library for scrolling through a page and handling events. 
 Meant to be used alongside StartApp.

## Usage

### Installation

```
$ elm package install abrykajlo/elm-scroll
```

## Examples

```
$ git clone https://github.com/abrykajlo/elm-scroll
$ cd elm-scroll/examples
$ make elm
```

Open `index.html` in your browser.

## Breaking down the example

First of all, the module is relying on the `window.onscroll` event, thus a port is needed:

### index.html

```html
<!DOCTYPE html>
<html>
<head>
	<title>Adam Brykajlo</title>
	<link rel="stylesheet" type="text/css" href="css/main.css">
	<script type="text/javascript" src="js/main.js"></script>
</head>
<body>
<div id="main"></div>
<script>
	var scroll = window.pageYOffset || document.body.scrollTop;
	var mountNode = document.getElementById('main');
	var myApp = Elm.Main.embed(mountNode);

	window.onscroll = function() {
		var newScroll = window.pageYOffset || document.body.scrollTop;
		myApp.ports.scroll.send([scroll, newScroll]);
		scroll = newScroll;
	};
</script>
</body>
</html>

```

### Elm files

Create a separate file for the *ports*:

```elm
-- Ports.elm
port module Ports exposing (..)

import Scroll exposing (Move)

port scroll : (Move -> msg) -> Sub msg
```

In your main file:

```elm
-- app definition
main =
    App.program
        { init = init
        , subscriptions = subscriptions
        , update = update
        , view = view
        }
```

Need to define at least two type of actions, one for the incoming port's messages and one for the animation frame:

```elm
type Action
    = Header Move             -- the message emitted by the input port
                              -- brings a tuple with previous and current scroll values
    | Shrink                  -- message to be sent when scrollTop < 400px
    | Grow                    -- message to be sent when scrollTop > 400px
    | Animate Animation.Msg   -- animation's tick
```

For the port to be handled, a new *subscriptions* has to be created:

```elm
subscriptions : Model -> Sub Action
subscriptions model =
    Sub.batch
        [ scroll Header
        , Animation.subscription Animate [ model.style ]
        ]
```

Finally, we need to handle properly all the messages:

```elm
update action model =
    case action of
        Grow ->
	    -- scrolling down, under 400px
	    -- ...
	    (model, Cmd.none)
        Shrink ->
	    -- scrolling up, over 400px
	    -- ...
	    (model, Cmd.none)
        Animate animMsg ->
	    -- here you can apply new styles, to be animated
	    --- ...
	    (model, Cmd.none)
        Header move ->
            Scroll.handle
                [ update Grow                   -- when scrollTop > 400px, send Grow message
                    |> Scroll.onCrossDown 400
                , update Shrink                 -- when scrollTop < 400px, send Shrink message
                    |> Scroll.onCrossUp 400
                ]
                move model
```

Enjoy!
