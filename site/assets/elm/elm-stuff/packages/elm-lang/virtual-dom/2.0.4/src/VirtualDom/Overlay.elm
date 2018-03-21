module VirtualDom.Overlay exposing
  ( State, none, corruptImport, badMetadata
  , Msg, close, assessImport
  , isBlocking
  , Config
  , Block
  , view
  , viewImportExport
  )

import Json.Decode as Decode
import Json.Encode as Encode
import VirtualDom.Helpers exposing (..)
import VirtualDom.Metadata as Metadata exposing (Metadata)
import VirtualDom.Report as Report exposing (Report)



type State
  = None
  | BadMetadata Metadata.Error
  | BadImport Report
  | RiskyImport Report Encode.Value


none : State
none =
  None


corruptImport : State
corruptImport =
  BadImport Report.CorruptHistory


badMetadata : Metadata.Error -> State
badMetadata =
  BadMetadata


isBlocking : State -> Bool
isBlocking state =
  case state of
    None ->
      False

    _ ->
      True



--  UPDATE


type Msg = Cancel | Proceed


close : Msg -> State -> Maybe Encode.Value
close msg state =
  case state of
    None ->
      Nothing

    BadMetadata _ ->
      Nothing

    BadImport _ ->
      Nothing

    RiskyImport _ rawHistory ->
      case msg of
        Cancel ->
          Nothing

        Proceed ->
          Just rawHistory


assessImport : Metadata -> String -> Result State Encode.Value
assessImport metadata jsonString =
  case Decode.decodeString uploadDecoder jsonString of
    Err _ ->
      Err corruptImport

    Ok (foreignMetadata, rawHistory) ->
      let
        report =
          Metadata.check foreignMetadata metadata
      in
        case Report.evaluate report of
          Report.Impossible ->
            Err (BadImport report)

          Report.Risky ->
            Err (RiskyImport report rawHistory)

          Report.Fine ->
            Ok rawHistory


uploadDecoder : Decode.Decoder (Metadata, Encode.Value)
uploadDecoder =
  Decode.map2 (,)
    (Decode.field "metadata" Metadata.decoder)
    (Decode.field "history" Decode.value)



-- VIEW


type alias Config msg =
  { resume : msg
  , open : msg
  , importHistory : msg
  , exportHistory : msg
  , wrap : Msg -> msg
  }


type Block = Normal | Pause | Message


view : Config msg -> Bool -> Bool -> Int -> State -> ( Block, Node msg )
view config isPaused isOpen numMsgs state =
  let
    (block, nodes) =
      viewHelp config isPaused isOpen numMsgs state
  in
    ( block
    , div [ class "elm-overlay" ] (styles :: nodes)
    )


viewHelp : Config msg -> Bool -> Bool -> Int -> State -> ( Block, List (Node msg) )
viewHelp config isPaused isOpen numMsgs state =
  case state of
    None ->
      let
        miniControls =
          if isOpen then [] else [ viewMiniControls config numMsgs ]
      in
        ( if isPaused then Pause else Normal
        , if isPaused && not isOpen then
            viewResume config :: miniControls
          else
            miniControls
        )

    BadMetadata badMetadata ->
      viewMessage config
        "Cannot use Import or Export"
        (viewBadMetadata badMetadata)
        (Accept "Ok")

    BadImport report ->
      viewMessage config
        "Cannot Import History"
        (viewReport True report)
        (Accept "Ok")

    RiskyImport report _ ->
      viewMessage config
        "Warning"
        (viewReport False report)
        (Choose "Cancel" "Import Anyway")


viewResume config =
  div [ class "elm-overlay-resume", onClick config.resume ]
    [ div [class "elm-overlay-resume-words"] [text "Click to Resume"] ]



-- VIEW MESSAGE


viewMessage : Config msg -> String -> List (Node msg) -> Buttons -> ( Block, List (Node msg) )
viewMessage config title details buttons =
  ( Message
  , [ div [ class "elm-overlay-message" ]
        [ div [ class "elm-overlay-message-title" ] [ text title ]
        , div [ class "elm-overlay-message-details" ] details
        , map config.wrap (viewButtons buttons)
        ]
    ]
  )


viewReport : Bool -> Report -> List (Node msg)
viewReport isBad report =
  case report of
    Report.CorruptHistory ->
      [ text "Looks like this history file is corrupt. I cannot understand it."
      ]

    Report.VersionChanged old new ->
      [ text <|
          "This history was created with Elm "
          ++ old ++ ", but you are using Elm "
          ++ new ++ " right now."
      ]

    Report.MessageChanged old new ->
      [ text <|
          "To import some other history, the overall message type must"
          ++ " be the same. The old history has "
      , viewCode old
      , text " messages, but the new program works with "
      , viewCode new
      , text " messages."
      ]

    Report.SomethingChanged changes ->
      [ node "p" [] [ text (if isBad then explanationBad else explanationRisky) ]
      , node "ul" [] (List.map viewChange changes)
      ]


explanationBad : String
explanationBad = """
The messages in this history do not match the messages handled by your
program. I noticed changes in the following types:
"""

explanationRisky : String
explanationRisky = """
This history seems old. It will work with this program, but some
messages have been added since the history was created:
"""


viewCode : String -> Node msg
viewCode name =
  node "code" [] [ text name ]


viewChange : Report.Change -> Node msg
viewChange change =
  node "li" [] <|
    case change of
      Report.AliasChange name ->
        [ span [ class "elm-overlay-message-details-type" ] [ viewCode name ]
        ]

      Report.UnionChange name { removed, changed, added, argsMatch } ->
        [ span [ class "elm-overlay-message-details-type" ] [ viewCode name ]
        , node "ul" []
            [ viewMention removed "Removed "
            , viewMention changed "Changed "
            , viewMention added "Added "
            ]
        , if argsMatch then
            text ""
          else
            text "This may be due to the fact that the type variable names changed."
        ]


viewMention : List String -> String -> Node msg
viewMention tags verbed =
  case List.map viewCode (List.reverse tags) of
    [] ->
      text ""

    [tag] ->
      node "li" []
        [ text verbed, tag, text "." ]

    [tag2, tag1] ->
      node "li" []
        [ text verbed, tag1, text " and ", tag2, text "." ]

    lastTag :: otherTags ->
      node "li" [] <|
        text verbed
        :: List.intersperse (text ", ") (List.reverse otherTags)
        ++ [ text ", and ", lastTag, text "." ]


viewBadMetadata : Metadata.Error -> List (Node msg)
viewBadMetadata {message, problems} =
  [ node "p" []
      [ text "The "
      , viewCode message
      , text " type of your program cannot be reliably serialized for history files."
      ]
  , node "p" [] [ text "Functions cannot be serialized, nor can values that contain functions. This is a problem in these places:" ]
  , node "ul" [] (List.map viewProblemType problems)
  , node "p" []
      [ text goodNews1
      , a [ href "https://guide.elm-lang.org/types/union_types.html" ] [ text "union types" ]
      , text ", in your messages. From there, your "
      , viewCode "update"
      , text goodNews2
      ]
  ]


goodNews1 = """
The good news is that having values like this in your message type is not
so great in the long run. You are better off using simpler data, like
"""


goodNews2 = """
function can pattern match on that data and call whatever functions, JSON
decoders, etc. you need. This makes the code much more explicit and easy to
follow for other readers (or you in a few months!)
"""


viewProblemType : Metadata.ProblemType -> Node msg
viewProblemType { name, problems } =
  node "li" []
    [ viewCode name
    , text (" can contain " ++ addCommas (List.map problemToString problems) ++ ".")
    ]


problemToString : Metadata.Problem -> String
problemToString problem =
  case problem of
    Metadata.Function ->
      "functions"

    Metadata.Decoder ->
      "JSON decoders"

    Metadata.Task ->
      "tasks"

    Metadata.Process ->
      "processes"

    Metadata.Socket ->
      "web sockets"

    Metadata.Request ->
      "HTTP requests"

    Metadata.Program ->
      "programs"

    Metadata.VirtualDom ->
      "virtual DOM values"


addCommas : List String -> String
addCommas items =
  case items of
    [] ->
      ""

    [item] ->
      item

    [item1, item2] ->
      item1 ++ " and " ++ item2

    lastItem :: otherItems ->
      String.join ", " (otherItems ++ [ " and " ++ lastItem ])



-- VIEW MESSAGE BUTTONS


type Buttons
  = Accept String
  | Choose String String


viewButtons : Buttons -> Node Msg
viewButtons buttons =
  div [ class "elm-overlay-message-buttons" ] <|
    case buttons of
      Accept proceed ->
        [ node "button" [ onClick Proceed ] [ text proceed ]
        ]

      Choose cancel proceed ->
        [ node "button" [ onClick Cancel ] [ text cancel ]
        , node "button" [ onClick Proceed ] [ text proceed ]
        ]



-- VIEW MINI CONTROLS


viewMiniControls : Config msg -> Int -> Node msg
viewMiniControls config numMsgs =
  div
    [ class "elm-mini-controls"
    ]
    [ div
        [ onClick config.open
        , class "elm-mini-controls-button"
        ]
        [ text ("Explore History (" ++ toString numMsgs ++ ")")
        ]
    , viewImportExport
        [class "elm-mini-controls-import-export"]
        config.importHistory
        config.exportHistory
    ]


viewImportExport : List (Property msg) -> msg -> msg -> Node msg
viewImportExport props importMsg exportMsg =
  div
    props
    [ button importMsg "Import"
    , text " / "
    , button exportMsg "Export"
    ]


button : msg -> String -> Node msg
button msg label =
  span [ onClick msg, style [("cursor","pointer")] ] [ text label ]



-- STYLE


styles : Node msg
styles =
  node "style" [] [ text """

.elm-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  color: white;
  pointer-events: none;
  font-family: 'Trebuchet MS', 'Lucida Grande', 'Bitstream Vera Sans', 'Helvetica Neue', sans-serif;
}

.elm-overlay-resume {
  width: 100%;
  height: 100%;
  cursor: pointer;
  text-align: center;
  pointer-events: auto;
  background-color: rgba(200, 200, 200, 0.7);
}

.elm-overlay-resume-words {
  position: absolute;
  top: calc(50% - 40px);
  font-size: 80px;
  line-height: 80px;
  height: 80px;
  width: 100%;
}

.elm-mini-controls {
  position: fixed;
  bottom: 0;
  right: 6px;
  border-radius: 4px;
  background-color: rgb(61, 61, 61);
  font-family: monospace;
  pointer-events: auto;
}

.elm-mini-controls-button {
  padding: 6px;
  cursor: pointer;
  text-align: center;
  min-width: 24ch;
}

.elm-mini-controls-import-export {
  padding: 4px 0;
  font-size: 0.8em;
  text-align: center;
  background-color: rgb(50, 50, 50);
}

.elm-overlay-message {
  position: absolute;
  width: 600px;
  height: 100%;
  padding-left: calc(50% - 300px);
  padding-right: calc(50% - 300px);
  background-color: rgba(200, 200, 200, 0.7);
  pointer-events: auto;
}

.elm-overlay-message-title {
  font-size: 36px;
  height: 80px;
  background-color: rgb(50, 50, 50);
  padding-left: 22px;
  vertical-align: middle;
  line-height: 80px;
}

.elm-overlay-message-details {
  padding: 8px 20px;
  overflow-y: auto;
  max-height: calc(100% - 156px);
  background-color: rgb(61, 61, 61);
}

.elm-overlay-message-details-type {
  font-size: 1.5em;
}

.elm-overlay-message-details ul {
  list-style-type: none;
  padding-left: 20px;
}

.elm-overlay-message-details ul ul {
  list-style-type: disc;
  padding-left: 2em;
}

.elm-overlay-message-details li {
  margin: 8px 0;
}

.elm-overlay-message-buttons {
  height: 60px;
  line-height: 60px;
  text-align: right;
  background-color: rgb(50, 50, 50);
}

.elm-overlay-message-buttons button {
  margin-right: 20px;
}

""" ]