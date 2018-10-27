effect module Navigation where { command = MyCmd, subscription = MySub } exposing
  ( back, forward
  , load, reload, reloadAndSkipCache
  , newUrl, modifyUrl
  , program, programWithFlags
  , Location
  )

{-| This is a library for managing browser navigation yourself.

The core functionality is the ability to &ldquo;navigate&rdquo; to new URLs,
changing the address bar of the browser *without* the browser kicking off a
request to your servers. Instead, you manage the changes yourself in Elm.


# Change the URL
@docs newUrl, modifyUrl

# Navigation
@docs back, forward

# Force Page Loads
@docs load, reload, reloadAndSkipCache

# Programs with Locations
@docs program, programWithFlags, Location

-}


import Dom.LowLevel exposing (onWindow)
import Html exposing (Html)
import Json.Decode as Json
import Native.Navigation
import Process
import Task exposing (Task)



-- PROGRAMS


{-| Same as [`Html.program`][doc], but your `update` function gets messages
whenever the URL changes.

[doc]: http://package.elm-lang.org/packages/elm-lang/html/latest/Html#program

The first difference is the `Location -> msg` argument. This converts a
[`Location`](#location) into a message whenever the URL changes. That message
is fed into your `update` function just like any other one.

The second difference is that the `init` function takes `Location` as an
argument. This lets you use the URL on the first frame.

**Note:** A location message is produced every time the URL changes. This
includes things exposed by this library, like `back` and `newUrl`, as well as
whenever the user clicks the back or forward buttons of the browsers. So if
the URL changes, you will hear about it in your `update` function.
-}
program
  : (Location -> msg)
  ->
    { init : Location -> (model, Cmd msg)
    , update : msg -> model -> (model, Cmd msg)
    , view : model -> Html msg
    , subscriptions : model -> Sub msg
    }
  -> Program Never model msg
program locationToMessage stuff =
  let
    subs model =
      Sub.batch
        [ subscription (Monitor locationToMessage)
        , stuff.subscriptions model
        ]

    init =
      stuff.init (Native.Navigation.getLocation ())
  in
    Html.program
      { init = init
      , view = stuff.view
      , update = stuff.update
      , subscriptions = subs
      }


{-| Works the same as [`program`](#program), but it can also handle flags.
See [`Html.programWithFlags`][doc] for more information.

[doc]: http://package.elm-lang.org/packages/elm-lang/html/latest/Html#programWithFlags
-}
programWithFlags
  : (Location -> msg)
  ->
    { init : flags -> Location -> (model, Cmd msg)
    , update : msg -> model -> (model, Cmd msg)
    , view : model -> Html msg
    , subscriptions : model -> Sub msg
    }
  -> Program flags model msg
programWithFlags locationToMessage stuff =
  let
    subs model =
      Sub.batch
        [ subscription (Monitor locationToMessage)
        , stuff.subscriptions model
        ]

    init flags =
      stuff.init flags (Native.Navigation.getLocation ())
  in
    Html.programWithFlags
      { init = init
      , view = stuff.view
      , update = stuff.update
      , subscriptions = subs
      }



-- TIME TRAVEL


{-| Go back some number of pages. So `back 1` goes back one page, and `back 2`
goes back two pages.

**Note:** You only manage the browser history that *you* created. Think of this
library as letting you have access to a small part of the overall history. So
if you go back farther than the history you own, you will just go back to some
other website!
-}
back : Int -> Cmd msg
back n =
  command (Jump -n)


{-| Go forward some number of pages. So `forward 1` goes forward one page, and
`forward 2` goes forward two pages. If there are no more pages in the future,
this will do nothing.

**Note:** You only manage the browser history that *you* created. Think of this
library as letting you have access to a small part of the overall history. So
if you go forward farther than the history you own, the user will end up on
whatever website they visited next!
-}
forward : Int -> Cmd msg
forward n =
  command (Jump n)


{-| Leave the current page and load the given URL. **This always results in a
page load**, even if the provided URL is the same as the current one.

    load "http://elm-lang.org"

Use [`newUrl`](#newUrl) and [`modifyUrl`](#modifyUrl) if you want to change
the URL without a page load.
-}
load : String -> Cmd msg
load url =
  command (Visit url)


{-| Reload the current page. **This always results in a page load!**
This may grab resources from the browser cache, so use
[`reloadAndSkipCache`](reloadAndSkipCache) if you want to be sure
that you are not loading any cached resources.
-}
reload : Cmd msg
reload =
  command (Reload False)


{-| Reload the current page without using the browser cache. **This always
results in a page load!** It is more common to want [`reload`](reload).
-}
reloadAndSkipCache : Cmd msg
reloadAndSkipCache =
  command (Reload True)



-- CHANGE HISTORY


{-| Step to a new URL. This will add a new entry to the browser history.

**Note:** If the user has gone `back` a few pages, there will be &ldquo;future
pages&rdquo; that the user can go `forward` to. Adding a new URL in that
scenario will clear out any future pages. It is like going back in time and
making a different choice.
-}
newUrl : String -> Cmd msg
newUrl url =
  command (New url)


{-| Modify the current URL. This *will not* add a new entry to the browser
history. It just changes the one you are on right now.
-}
modifyUrl : String -> Cmd msg
modifyUrl url =
  command (Modify url)



-- LOCATION


{-| A bunch of information about the address bar.

**Note 1:** Almost everyone will want to use a URL parsing library like
[`evancz/url-parser`][parse] to turn a `Location` into something more useful
in your `update` function.

[parse]: https://github.com/evancz/url-parser

**Note 2:** These fields correspond exactly with the fields of `document.location`
as described [here](https://developer.mozilla.org/en-US/docs/Web/API/Location).
-}
type alias Location =
  { href : String
  , host : String
  , hostname : String
  , protocol : String
  , origin : String
  , port_ : String
  , pathname : String
  , search : String
  , hash : String
  , username : String
  , password : String
  }


-- EFFECT MANAGER


type MyCmd msg
  = Jump Int
  | New String
  | Modify String
  | Visit String
  | Reload Bool


cmdMap : (a -> b) -> MyCmd a -> MyCmd b
cmdMap _ myCmd =
  case myCmd of
    Jump n ->
      Jump n

    New url ->
      New url

    Modify url ->
      Modify url

    Visit url ->
        Visit url

    Reload skipCache ->
        Reload skipCache


type MySub msg =
  Monitor (Location -> msg)


subMap : (a -> b) -> MySub a -> MySub b
subMap func (Monitor tagger) =
  Monitor (tagger >> func)



-- STATE


type alias State msg =
  { subs : List (MySub msg)
  , popWatcher : Maybe PopWatcher
  }


type PopWatcher
  = Normal Process.Id
  | InternetExplorer Process.Id Process.Id



-- INIT


init : Task Never (State msg)
init =
  Task.succeed (State [] Nothing)



-- SELF MESSAGES


onSelfMsg : Platform.Router msg Location -> Location -> State msg -> Task Never (State msg)
onSelfMsg router location state =
  notify router state.subs location
    &> Task.succeed state


(&>) task1 task2 =
  Task.andThen (\_ -> task2) task1



-- APP MESSAGES


onEffects : Platform.Router msg Location -> List (MyCmd msg) -> List (MySub msg) -> State msg -> Task Never (State msg)
onEffects router cmds subs {popWatcher} =
  let
    stepState =
      case (subs, popWatcher) of
        ([], Just watcher) ->
          killPopWatcher watcher
            &> Task.succeed (State subs Nothing)

        (_ :: _, Nothing) ->
          Task.map (State subs << Just) (spawnPopWatcher router)

        (_, _) ->
          Task.succeed (State subs popWatcher)

  in
    Task.sequence (List.map (cmdHelp router subs) cmds)
      &> stepState


cmdHelp : Platform.Router msg Location -> List (MySub msg) -> MyCmd msg -> Task Never ()
cmdHelp router subs cmd =
  case cmd of
    Jump n ->
      go n

    New url ->
      pushState url
        |> Task.andThen (notify router subs)

    Modify url ->
      replaceState url
        |> Task.andThen (notify router subs)

    Visit url ->
      setLocation url

    Reload skipCache ->
      reloadPage skipCache



notify : Platform.Router msg Location -> List (MySub msg) -> Location -> Task x ()
notify router subs location =
  let
    send (Monitor tagger) =
      Platform.sendToApp router (tagger location)
  in
    Task.sequence (List.map send subs)
      &> Task.succeed ()


setLocation : String -> Task x ()
setLocation =
  Native.Navigation.setLocation


reloadPage : Bool -> Task x ()
reloadPage =
  Native.Navigation.reloadPage


go : Int -> Task x ()
go =
  Native.Navigation.go


pushState : String -> Task x Location
pushState =
  Native.Navigation.pushState


replaceState : String -> Task x Location
replaceState =
  Native.Navigation.replaceState



-- POP WATCHER STUFF


spawnPopWatcher : Platform.Router msg Location -> Task x PopWatcher
spawnPopWatcher router =
  let
    reportLocation _ =
      Platform.sendToSelf router (Native.Navigation.getLocation ())
  in
    if Native.Navigation.isInternetExplorer11 () then
      Task.map2 InternetExplorer
        (Process.spawn (onWindow "popstate" Json.value reportLocation))
        (Process.spawn (onWindow "hashchange" Json.value reportLocation))

    else
      Task.map Normal <|
        Process.spawn (onWindow "popstate" Json.value reportLocation)



killPopWatcher : PopWatcher -> Task x ()
killPopWatcher popWatcher =
  case popWatcher of
    Normal pid ->
      Process.kill pid

    InternetExplorer pid1 pid2 ->
      Process.kill pid1
        &> Process.kill pid2
